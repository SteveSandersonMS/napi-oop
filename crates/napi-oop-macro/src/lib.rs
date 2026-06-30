//! The `#[napi]` attribute macro for `napi-oop`.
//!
//! One annotated source produces a **dual-ABI** result:
//!
//! - The real napi-rs `#[napi]` ABI is emitted (by delegating to
//!   `::napi_derive::napi`) so the cdylib loads in-process in Node as a normal
//!   napi addon.
//! - Out-of-process remoting glue is emitted alongside — a serde/wire-codec
//!   dispatch thunk plus a registry entry the runtime advertises to a Node child
//!   — so the same cdylib, loaded by a thin Rust provider exe, serves Node
//!   out-of-process.
//!
//! The user's source is identical; the macro generates both halves. Functions
//! with an `impl Fn(..)` parameter (which napi-derive cannot accept) get a
//! generated in-proc adapter taking a `ThreadsafeFunction` instead.

use proc_macro::TokenStream;

/// Drop-in replacement for napi-rs's `#[napi]`. Emits the real napi ABI plus the
/// out-of-process wire glue from one annotated item. See the crate docs.
#[proc_macro_attribute]
pub fn napi(attr: TokenStream, item: TokenStream) -> TokenStream {
    dual::expand(attr, item)
}

mod dual {
    use proc_macro::TokenStream;
    use quote::{format_ident, quote, ToTokens};
    use syn::{parse_macro_input, FnArg, ImplItem, Item, ItemFn, ItemImpl, ItemStruct, PatType};

    /// Dual-emit entry. Free fns get the real napi ABI (or an in-proc adapter for
    /// `impl Fn` params) plus a dispatch thunk + registration; class `impl` blocks
    /// get napi-derive's class glue plus a thunk per method/constructor; plain
    /// structs/objects/enums get napi-derive plus serde for the wire.
    pub(super) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
        // `#[napi(object)]` marks a plain value struct that crosses the boundary
        // by serde; everything else dispatches on the item kind.
        let attr_str = attr.to_string();
        let is_object = attr_str
            .split(|c: char| !c.is_alphanumeric())
            .any(|t| t == "object");
        let is_string_enum = attr_str.contains("string_enum");
        let js_name = js_name_from_tokens(attr.clone());
        let attr2: proc_macro2::TokenStream = attr.into();
        let parsed = parse_macro_input!(item as Item);
        match parsed {
            Item::Fn(func) => expand_fn(func, js_name, attr2),
            Item::Impl(imp) => expand_impl(imp, attr2),
            Item::Struct(s) if is_object => expand_object(s, attr2),
            Item::Struct(s) => expand_class_struct(s, js_name, attr2),
            // `#[napi(string_enum)]`: carried by serde as a string. Inject the
            // missing (de)serialization derives. Plain (int) `#[napi]` enums keep
            // napi-rs's numeric repr; both still get the real napi ABI.
            Item::Enum(e) if is_string_enum => expand_enum(e, attr2),
            Item::Enum(e) => quote!(#[::napi_derive::napi(#attr2)] #e).into(),
            other => quote!(#[::napi_derive::napi(#attr2)] #other).into(),
        }
    }

    /// Extract a `js_name = "…"` value from the `#[napi(…)]` *macro argument*
    /// tokens of a free function (e.g. `js_name = "fooBar"` in `#[napi(js_name =
    /// "fooBar")]`). Other meta items (`ts_args_type = …`, bare flags) are
    /// tolerated and ignored. Returns `None` when no `js_name` is present.
    fn js_name_from_tokens(attr: TokenStream) -> Option<String> {
        let mut found: Option<String> = None;
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("js_name") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                found = Some(lit.value());
            } else if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let _: syn::Expr = value.parse()?;
            }
            Ok(())
        });
        let _ = syn::parse::Parser::parse(parser, attr);
        found
    }

    /// Extract a `js_name = "…"` value from a method's inner `#[napi(…)]`
    /// attribute(s). Mirrors [`js_name_from_tokens`] for the `impl` path, where
    /// the attribute rides on the method rather than the macro invocation.
    fn js_name_from_attrs(attrs: &[syn::Attribute]) -> Option<String> {
        let mut found: Option<String> = None;
        for attr in attrs {
            if attr.path().is_ident("napi") {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("js_name") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        found = Some(lit.value());
                    } else if meta.input.peek(syn::Token![=]) {
                        let value = meta.value()?;
                        let _: syn::Expr = value.parse()?;
                    }
                    Ok(())
                });
            }
        }
        found
    }

    /// serde derives (camelCase fields, matching napi-rs's JS field naming) so the
    /// struct round-trips over the wire, and register its field shape so the TS
    /// generator emits a matching `interface` instead of `unknown`.
    fn expand_object(mut item: ItemStruct, attr2: proc_macro2::TokenStream) -> TokenStream {
        let name = item.ident.to_string();

        // Per field: strip napi-rs's `#[napi(..)]` (a non-existent attribute in
        // this build) and translate `js_name = "x"` into the equivalent serde
        // rename so the wire field name still matches what JS expects.
        let mut field_names = Vec::new();
        let mut field_types = Vec::new();
        if let syn::Fields::Named(fields) = &mut item.fields {
            for field in &mut fields.named {
                let Some(ident) = field.ident.clone() else {
                    continue;
                };
                field_names.push(ident.to_string());
                let ty = &field.ty;
                field_types.push(
                    quote!(#ty)
                        .to_string()
                        .split_whitespace()
                        .collect::<String>(),
                );

                let mut field_has_serde_rename = false;
                for attr in &field.attrs {
                    if attr.path().is_ident("serde") {
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("rename") {
                                field_has_serde_rename = true;
                            }
                            if meta.input.peek(syn::Token![=]) {
                                let value = meta.value()?;
                                let _: syn::Expr = value.parse()?;
                            }
                            Ok(())
                        });
                    }
                }

                let mut js_name: Option<String> = None;
                field.attrs.retain(|attr| {
                    if attr.path().is_ident("napi") {
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("js_name") {
                                let value = meta.value()?;
                                let lit: syn::LitStr = value.parse()?;
                                js_name = Some(lit.value());
                            } else if meta.input.peek(syn::Token![=]) {
                                let value = meta.value()?;
                                let _: syn::Expr = value.parse()?;
                            }
                            Ok(())
                        });
                        // Keep the field's `#[napi(..)]`: napi-derive (the active
                        // attribute macro once we attach it below) consumes it for
                        // the in-proc ABI before the serde derives run.
                        true
                    } else {
                        true
                    }
                });
                if let Some(rename) = js_name {
                    if !field_has_serde_rename {
                        field
                            .attrs
                            .push(syn::parse_quote!(#[serde(rename = #rename)]));
                    }
                }
            }
        }

        // Carry the value over the wire with serde: inject whatever derives /
        // container attributes the struct doesn't already declare (camelCase
        // field naming, matching napi-rs).
        inject_serde(&mut item.attrs, true);

        let expanded = quote! {
            #[::napi_derive::napi(#attr2)]
            #item

            const _: () = {
                ::napi_oop::inventory::submit! {
                    ::napi_oop::registry::RegisteredObject {
                        name: #name,
                        field_names: &[#(#field_names),*],
                        field_types: &[#(#field_types),*],
                    }
                }
            };
        };
        expanded.into()
    }

    fn expand_class_struct(
        item: ItemStruct,
        js_name: Option<String>,
        attr2: proc_macro2::TokenStream,
    ) -> TokenStream {
        let rust_name = item.ident.to_string();
        // A struct without `js_name` still gets the real napi class ABI; only the
        // codegen rename registration is conditional.
        let Some(js_name) = js_name else {
            return quote!(#[::napi_derive::napi(#attr2)] #item).into();
        };
        quote! {
            #[::napi_derive::napi(#attr2)]
            #item

            const _: () = {
                ::napi_oop::inventory::submit! {
                    ::napi_oop::registry::RegisteredClassRename {
                        rust_name: #rust_name,
                        js_name: #js_name,
                    }
                }
            };
        }
        .into()
    }

    /// A `#[napi(string_enum)]` (or other `#[napi]`) enum: a plain value carried
    /// by serde as a string. napi-rs derives its own (de)serialization; we inject
    /// whatever serde derives are missing so it round-trips. Variant-level
    /// `#[napi(..)]` attributes are stripped (not real attributes in this build),
    /// and `rename_all` is left to the source (its `string_enum = "…"` case maps
    /// to a matching `#[serde(rename_all = "…")]`), defaulting to verbatim variant
    /// names like napi-rs.
    fn expand_enum(mut item: syn::ItemEnum, attr2: proc_macro2::TokenStream) -> TokenStream {
        inject_serde(&mut item.attrs, false);
        quote! { #[::napi_derive::napi(#attr2)] #item }.into()
    }

    /// Inspect a type's existing container attributes and append whichever serde
    /// derives / container attributes are missing so it round-trips over the wire.
    /// Duplicating a `derive(Serialize)` or `serde(rename_all = …)` the source
    /// already declares would be a hard error, so each is added only if absent.
    /// `default_rename_all` injects `rename_all = "camelCase"` when the type has
    /// none (correct for objects; enums keep serde's verbatim default to match
    /// napi-rs string-enum naming).
    fn inject_serde(attrs: &mut Vec<syn::Attribute>, default_rename_all: bool) {
        let mut has_serialize = false;
        let mut has_deserialize = false;
        let mut has_rename_all = false;
        let mut has_serde_crate = false;
        for attr in attrs.iter() {
            if attr.path().is_ident("derive") {
                let _ = attr.parse_nested_meta(|meta| {
                    // Match the last path segment so qualified derives such as
                    // `serde::Serialize` count too.
                    let last = meta.path.segments.last().map(|s| s.ident.to_string());
                    if last.as_deref() == Some("Serialize") {
                        has_serialize = true;
                    }
                    if last.as_deref() == Some("Deserialize") {
                        has_deserialize = true;
                    }
                    Ok(())
                });
            } else if attr.path().is_ident("serde") {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("rename_all") {
                        has_rename_all = true;
                    }
                    if meta.path.is_ident("crate") {
                        has_serde_crate = true;
                    }
                    if meta.input.peek(syn::Token![=]) {
                        let value = meta.value()?;
                        let _: syn::Expr = value.parse()?;
                    }
                    Ok(())
                });
            }
        }

        let mut derives = Vec::new();
        if !has_serialize {
            derives.push(quote!(::napi_oop::serde::Serialize));
        }
        if !has_deserialize {
            derives.push(quote!(::napi_oop::serde::Deserialize));
        }
        let mut serde_args = Vec::new();
        if !has_serde_crate {
            serde_args.push(quote!(crate = "::napi_oop::serde"));
        }
        if default_rename_all && !has_rename_all {
            serde_args.push(quote!(rename_all = "camelCase"));
        }
        // Append (rather than prepend) the injected attributes so any derive the
        // type already carries precedes the serde helper attribute — emitting
        // `#[serde(..)]` before the `#[derive(Serialize)]` that introduces it is
        // a "derive helper attribute used before it is introduced" error.
        if !derives.is_empty() {
            attrs.push(syn::parse_quote!(#[derive(#(#derives),*)]));
        }
        if !serde_args.is_empty() {
            attrs.push(syn::parse_quote!(#[serde(#(#serde_args),*)]));
        }
    }

    fn expand_fn(
        func: ItemFn,
        js_name: Option<String>,
        attr2: proc_macro2::TokenStream,
    ) -> TokenStream {
        let fn_name = func.sig.ident.clone();
        let fn_name_str = fn_name.to_string();
        let js_name_str = js_name.unwrap_or_default();

        // Collect the (typed) argument types; methods aren't supported yet.
        let mut arg_types = Vec::new();
        let mut arg_names = Vec::new();
        for input in &func.sig.inputs {
            match input {
                FnArg::Typed(PatType { ty, pat, .. }) => {
                    arg_types.push((**ty).clone());
                    let name = match &**pat {
                        syn::Pat::Ident(p) => p.ident.to_string(),
                        _ => format!("arg{}", arg_types.len() - 1),
                    };
                    arg_names.push(name);
                }
                FnArg::Receiver(receiver) => {
                    return syn::Error::new_spanned(
                        receiver,
                        "#[napi] out-of-proc mode does not support methods (`self`) yet",
                    )
                    .to_compile_error()
                    .into();
                }
            }
        }

        let arity = arg_types.len();
        let arg_idents: Vec<_> = (0..arity).map(|i| format_ident!("__arg{i}")).collect();
        let decode_args = arg_idents.iter().zip(arg_types.iter()).map(decode_arg);

        // Stringify each Rust type for the manifest the TS generator consumes;
        // callback params (both forms) become a TS function-type string. Env
        // params are skipped so the surfaced signature matches the JS call.
        let param_type_strs: Vec<String> = arg_types
            .iter()
            .filter(|ty| !is_env_ty(ty))
            .map(|ty| {
                if let Some((inputs, _)) = fn_trait_sig(ty) {
                    ts_fn_type(&inputs)
                } else if let Some(inner) = tsfn_inner(ty) {
                    ts_tsfn_type(&inner)
                } else {
                    quote!(#ty).to_string().split_whitespace().collect()
                }
            })
            .collect();
        let arg_names: Vec<String> = arg_names
            .iter()
            .zip(arg_types.iter())
            .filter(|(_, ty)| !is_env_ty(ty))
            .map(|(n, _)| n.clone())
            .collect();
        let ret_ok_type = result_ok_type(&func.sig.output);
        let ret_type_str: String = match (&ret_ok_type, &func.sig.output) {
            (Some(ok), _) => quote!(#ok).to_string().split_whitespace().collect(),
            (None, syn::ReturnType::Type(_, ty)) => {
                quote!(#ty).to_string().split_whitespace().collect()
            }
            (None, syn::ReturnType::Default) => "()".to_string(),
        };

        // Async Rust fns surface as async on TS in *both* binding modes. The
        // dispatch thunk drives the future to completion; the manifest marks the
        // fn async so the generator emits `Promise<T>` even for the sync binding.
        let is_async = func.sig.asyncness.is_some();
        let call_args: Vec<_> = arg_idents
            .iter()
            .zip(arg_types.iter())
            .map(|(id, ty)| call_arg_token(id, ty))
            .collect();
        let call_expr = if is_async {
            quote! { ::napi_oop::block_on(#fn_name(#(#call_args),*)) }
        } else {
            quote! { #fn_name(#(#call_args),*) }
        };

        // A `Result<T, E>` Err maps to an error reply (mirroring napi-rs's throw);
        // a plain return is always success. The Ok value / plain value is encoded
        // by the return-encoder, which mints class instances and serializes the rest.
        let encode_ret = encode_owned(&ret_ok_type);

        // In-proc napi ABI. Some param shapes napi-derive can't accept verbatim:
        // an `impl Fn(..)` trait object, or a `&External<T>` borrow (napi-rs has no
        // `FromNapiRef for External`). For those we emit the plain fn plus a thin
        // adapter that takes napi-friendly params (a real `ThreadsafeFunction` / an
        // owned `External`) and forwards them. Every other fn gets napi-derive
        // attached straight onto it.
        let needs_adapter = func.sig.inputs.iter().any(|a| {
            matches!(a, FnArg::Typed(pt)
                if fn_trait_sig(&pt.ty).is_some() || external_ref_inner(&pt.ty).is_some())
        });
        let ip_js_name = if js_name_str.is_empty() {
            camel(&fn_name_str)
        } else {
            js_name_str.clone()
        };
        let napi_abi = if needs_adapter {
            build_inproc_fn_adapter(&func, &ip_js_name)
        } else {
            quote! { #[::napi_derive::napi(#attr2)] #func }
        };

        let expanded = quote! {
            #napi_abi

            const _: () = {
                fn __napi_oop_dispatch(
                    __args: ::std::vec::Vec<::napi_oop::rmpv::Value>,
                    __cb: &::std::sync::Arc<dyn ::napi_oop::registry::Callbacks>,
                ) -> ::core::result::Result<::napi_oop::rmpv::Value, ::std::string::String> {
                    // napi-rs treats trailing args as optional: a caller may omit
                    // trailing `Option<T>` arguments (they arrive as `undefined`,
                    // decoded here as nil -> `None`) and any extra args are ignored.
                    // So we don't enforce an exact arity — `decode_arg` pads missing
                    // trailing args with nil and the iterator simply drops extras.
                    let mut __iter = __args.into_iter();
                    #(#decode_args)*
                    let __ret = #call_expr;
                    #encode_ret
                }

                ::napi_oop::inventory::submit! {
                    ::napi_oop::registry::RegisteredFn {
                        name: #fn_name_str,
                        js_name: #js_name_str,
                        dispatch: __napi_oop_dispatch,
                        params: &[#(#param_type_strs),*],
                        param_names: &[#(#arg_names),*],
                        ret: #ret_type_str,
                        is_async: #is_async,
                    }
                }
            };
        };

        expanded.into()
    }

    /// Build the in-proc napi adapter for a free fn whose signature napi-derive
    /// can't accept verbatim: an `impl Fn(T)` callback (forwarded as a real
    /// `ThreadsafeFunction<T>`) or a `&External<T>` borrow (taken by value and
    /// lent). Returns the plain original fn plus the napi-annotated adapter.
    fn build_inproc_fn_adapter(func: &ItemFn, ip_js_name: &str) -> proc_macro2::TokenStream {
        let fn_name = func.sig.ident.clone();
        let adapter_ident = format_ident!("__napi_oop_ip_{}", fn_name);
        let asyncness = &func.sig.asyncness;
        let is_async = func.sig.asyncness.is_some();
        let output = &func.sig.output;

        let mut params = Vec::new();
        let mut call_args = Vec::new();
        for input in &func.sig.inputs {
            match input {
                FnArg::Typed(pt) => {
                    let ident = match &*pt.pat {
                        syn::Pat::Ident(p) => p.ident.clone(),
                        _ => {
                            return syn::Error::new_spanned(
                                &pt.pat,
                                "#[napi] in-proc adapter requires named parameters",
                            )
                            .to_compile_error();
                        }
                    };
                    if let Some((inputs, _out)) = fn_trait_sig(&pt.ty) {
                        if inputs.len() != 1 {
                            return syn::Error::new_spanned(
                                &pt.ty,
                                "#[napi] callbacks support a single-argument `impl Fn(T)` only",
                            )
                            .to_compile_error();
                        }
                        let cb_arg = &inputs[0];
                        if is_async {
                            // An async fn may invoke the callback from a worker
                            // thread (the future runs off the JS thread), so the
                            // in-proc adapter must use a real `ThreadsafeFunction`
                            // — exactly what traditional napi requires for a
                            // cross-thread callback. Calls queue onto the host's
                            // event loop (fire-and-forget), matching napi's TSFN.
                            // `CalleeHandled = false` so the JS callback receives
                            // just `(value)` — consistent with the sync `impl Fn`
                            // form, which delivers a single argument inline.
                            params.push(quote! {
                                #ident: ::napi_oop::ThreadsafeFunction<
                                    #cb_arg,
                                    ::napi::bindgen_prelude::Unknown<'static>,
                                    #cb_arg,
                                    ::napi::Status,
                                    false,
                                >
                            });
                            call_args.push(quote! {
                                move |__cb0: #cb_arg| {
                                    #ident.call(
                                        __cb0,
                                        ::napi_oop::ThreadsafeFunctionCallMode::NonBlocking,
                                    );
                                }
                            });
                        } else {
                            // A sync fn runs on the JS thread, so the callback is
                            // invoked inline during the call. Traditional napi
                            // models this as a synchronous `Function` (called on
                            // the same thread, delivered before the call returns),
                            // NOT a `ThreadsafeFunction` (which would defer the
                            // call until the JS thread next yields — diverging from
                            // both traditional napi and the out-of-process path,
                            // where the blocking provider delivers callbacks before
                            // the sync call returns).
                            params.push(quote! {
                                #ident: ::napi::bindgen_prelude::Function<#cb_arg, ()>
                            });
                            call_args.push(quote! {
                                move |__cb0: #cb_arg| {
                                    let _ = #ident.call(__cb0);
                                }
                            });
                        }
                    } else if let Some(owned) = external_ref_inner(&pt.ty) {
                        // napi-rs has no `FromNapiRef for External`; take the handle
                        // by value (decoded via `FromNapiValue`) and lend it.
                        params.push(quote! { #ident: #owned });
                        call_args.push(quote! { &#ident });
                    } else {
                        params.push(quote! { #input });
                        call_args.push(quote! { #ident });
                    }
                }
                FnArg::Receiver(r) => {
                    return syn::Error::new_spanned(
                        r,
                        "#[napi] out-of-proc mode does not support methods (`self`) yet",
                    )
                    .to_compile_error();
                }
            }
        }

        let await_tok = if asyncness.is_some() {
            quote!(.await)
        } else {
            quote!()
        };

        quote! {
            #func

            #[::napi_derive::napi(js_name = #ip_js_name)]
            pub #asyncness fn #adapter_ident(#(#params),*) #output {
                #fn_name(#(#call_args),*)#await_tok
            }
        }
    }

    /// keyed `Class.method`. The constructor builds the value into the object
    /// slab and returns its top-level handle; methods take the handle as the
    /// first wire arg, borrow the value, and call. Class metadata is registered
    /// so the generator emits a TS class proxy.
    fn expand_impl(imp: ItemImpl, attr2: proc_macro2::TokenStream) -> TokenStream {
        let self_ty = &imp.self_ty;
        let class_name = match &**self_ty {
            syn::Type::Path(p) => p.path.segments.last().unwrap().ident.to_string(),
            _ => {
                return syn::Error::new_spanned(self_ty, "#[napi] impl: unsupported self type")
                    .to_compile_error()
                    .into()
            }
        };
        let mut thunks = Vec::new();
        for item in &imp.items {
            let ImplItem::Fn(method) = item else { continue };
            let is_napi = method.attrs.iter().any(|a| a.path().is_ident("napi"));
            if !is_napi {
                continue;
            }
            let is_ctor = method.attrs.iter().any(|a| {
                a.path().is_ident("napi")
                    && a.meta.to_token_stream().to_string().contains("constructor")
            });
            let is_getter = method.attrs.iter().any(|a| {
                a.path().is_ident("napi") && a.meta.to_token_stream().to_string().contains("getter")
            });
            thunks.push(method_thunk(
                &class_name,
                self_ty,
                method,
                is_ctor,
                is_getter,
            ));
        }
        // Re-emit the impl untouched with napi-derive attached: it owns the
        // in-proc class ABI and consumes the inner `#[napi(constructor/getter/
        // js_name)]` helper attrs (which we also read above to drive the wire
        // thunks). We must NOT strip those attrs anymore — napi-derive needs them.
        // A class instance crosses the wire as an external handle: any fn returning
        // a class (its own, another class, or a free-fn factory) mints the owned
        // instance into the slab via the return-encoder. Marking the type
        // `NapiClass` is what lets that encoder recognise it by type — no `Clone`
        // or `Serialize` on the class is required, since the instance is moved
        // (never copied or field-serialized) into the slab.
        let class_marker = quote! {
            impl ::napi_oop::types::NapiClass for #self_ty {}
        };
        quote! {
            #[::napi_derive::napi(#attr2)]
            #imp

            #class_marker
            #(#thunks)*
        }
        .into()
    }

    /// Build the dispatch thunk + registration for one constructor/method/getter.
    fn method_thunk(
        class: &str,
        self_ty: &syn::Type,
        method: &syn::ImplItemFn,
        is_ctor: bool,
        is_getter: bool,
    ) -> proc_macro2::TokenStream {
        let m_ident = method.sig.ident.clone();
        let js_method = if is_ctor {
            "constructor".to_string()
        } else if let Some(jn) = js_name_from_attrs(&method.attrs) {
            jn
        } else {
            camel(&m_ident.to_string())
        };
        let wire_name = format!("{class}.{}", m_ident);
        let dispatch_ident = format_ident!("__napi_oop_m_{}_{}", class.to_lowercase(), m_ident);

        let mut arg_types = Vec::new();
        let mut arg_names = Vec::new();
        for input in &method.sig.inputs {
            if let FnArg::Typed(PatType { ty, pat, .. }) = input {
                arg_types.push((**ty).clone());
                arg_names.push(match &**pat {
                    syn::Pat::Ident(p) => p.ident.to_string(),
                    _ => format!("arg{}", arg_types.len() - 1),
                });
            }
        }
        let arity = arg_types.len();
        let arg_idents: Vec<_> = (0..arity).map(|i| format_ident!("__arg{i}")).collect();
        let decode = arg_idents.iter().zip(arg_types.iter()).map(decode_arg);
        let call_args: Vec<_> = arg_idents
            .iter()
            .zip(arg_types.iter())
            .map(|(id, ty)| call_arg_token(id, ty))
            .collect();
        // Env params are host-injected: omit them from the manifest signature.
        let param_strs: Vec<String> = arg_types
            .iter()
            .filter(|ty| !is_env_ty(ty))
            .map(|ty| ts_param(ty))
            .collect();
        let arg_names: Vec<String> = arg_names
            .iter()
            .zip(arg_types.iter())
            .filter(|(_, ty)| !is_env_ty(ty))
            .map(|(n, _)| n.clone())
            .collect();
        let is_async = method.sig.asyncness.is_some();
        let ret_ok = result_ok_type(&method.sig.output);
        let ret_str: String = match (&ret_ok, &method.sig.output) {
            (Some(ok), _) => quote!(#ok).to_string().split_whitespace().collect(),
            (None, syn::ReturnType::Type(_, ty)) => {
                quote!(#ty).to_string().split_whitespace().collect()
            }
            (None, syn::ReturnType::Default) => "()".to_string(),
        };

        let mut ret_label = ret_str.clone();
        if ret_str == "Self" {
            ret_label = class.to_string();
        }
        // Receiver count: a constructor has none; methods take handle first.
        let takes_self = matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)));
        let m_ident2 = m_ident.clone();
        // napi-rs requires `&mut self` async methods to be `unsafe`; honour that
        // keyword on the wire side too by wrapping the call. `block_on` then drives
        // the (already produced) future for async methods.
        let is_unsafe = method.sig.unsafety.is_some();
        let wrap_call = |inner: proc_macro2::TokenStream| -> proc_macro2::TokenStream {
            let inner = if is_unsafe {
                quote!(unsafe { #inner })
            } else {
                inner
            };
            if is_async {
                quote!(::napi_oop::block_on(#inner))
            } else {
                inner
            }
        };
        let body = if is_ctor {
            let call = wrap_call(quote!(#self_ty::#m_ident2(#(#call_args),*)));
            let wrap = if ret_ok.is_some() {
                quote!(match __ret {
                    Ok(v) => v,
                    Err(e) => return Err(::std::string::ToString::to_string(&e)),
                })
            } else {
                quote!(__ret)
            };
            quote! {
                let mut __iter = __args.into_iter();
                #(#decode)*
                let __ret = #call;
                let __obj = #wrap;
                let __tok = ::napi_oop::types::object_new(::std::boxed::Box::new(__obj));
                ::core::result::Result::Ok(::napi_oop::wire::external_marker(__tok))
            }
        } else if takes_self {
            let call = wrap_call(quote!(__self.#m_ident2(#(#call_args),*)));
            // Move the owned return out of `with_object` first, then encode: the
            // encoder may mint into the slab (for class returns), which would
            // re-enter the slab mutex and deadlock if done inside the closure.
            let encode = encode_owned(&ret_ok);
            quote! {
                let mut __iter = __args.into_iter();
                let __tok = ::napi_oop::wire::external_handle(&__iter.next().ok_or("missing receiver handle")?)?;
                #(#decode)*
                let __ret = ::napi_oop::types::with_object::<#self_ty,_>(__tok, |__self| #call).ok_or("object handle no longer live")?;
                #encode
            }
        } else {
            // associated fn (static) — treat like a free fn
            let call = wrap_call(quote!(#self_ty::#m_ident2(#(#call_args),*)));
            let encode = encode_owned(&ret_ok);
            quote! { let mut __iter = __args.into_iter(); #(#decode)* let __ret = #call; #encode }
        };

        quote! {
            const _: () = {
                fn #dispatch_ident(__args: ::std::vec::Vec<::napi_oop::rmpv::Value>, __cb: &::std::sync::Arc<dyn ::napi_oop::registry::Callbacks>) -> ::core::result::Result<::napi_oop::rmpv::Value, ::std::string::String> {
                    #body
                }
                ::napi_oop::inventory::submit! { ::napi_oop::registry::RegisteredFn { name: #wire_name, js_name: "", dispatch: #dispatch_ident, params: &[#(#param_strs),*], param_names: &[#(#arg_names),*], ret: #ret_label, is_async: #is_async } }
                ::napi_oop::inventory::submit! { ::napi_oop::registry::RegisteredMethod { class: #class, method: #js_method, rust_name: #wire_name, params: &[#(#param_strs),*], param_names: &[#(#arg_names),*], ret: #ret_label, is_async: #is_async, is_getter: #is_getter } }
            };
        }
    }

    fn camel(name: &str) -> String {
        let mut out = String::new();
        let mut up = false;
        for c in name.chars() {
            if c == '_' {
                up = true;
            } else if up {
                out.extend(c.to_uppercase());
                up = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Encode an owned provider return (`__ret`) for the wire. A `Result` is
    /// unwrapped — `Err` becomes an error reply (mirroring napi-rs's throw) — and
    /// the success value is handed to the return-encoder, which mints class
    /// instances into the slab as external handles and serializes everything else.
    fn encode_owned(ret_ok: &Option<syn::Type>) -> proc_macro2::TokenStream {
        if ret_ok.is_some() {
            quote! {
                match __ret {
                    ::core::result::Result::Ok(__v) => ::napi_oop::__napi_oop_encode_return!(__v),
                    ::core::result::Result::Err(__e) => {
                        // napi-derive requires the Err type to be `napi::Error`; on
                        // the wire we surface the same `reason` string Node would
                        // see thrown in-proc (not the `Display` "<Status>, reason").
                        let __e: ::napi_oop::Error = ::core::convert::Into::into(__e);
                        ::core::result::Result::Err(::std::clone::Clone::clone(&__e.reason))
                    }
                }
            }
        } else {
            quote! { ::napi_oop::__napi_oop_encode_return!(__ret) }
        }
    }

    fn ts_param(ty: &syn::Type) -> String {
        if let Some((inputs, _)) = fn_trait_sig(ty) {
            ts_fn_type(&inputs)
        } else if let Some(inner) = tsfn_inner(ty) {
            ts_tsfn_type(&inner)
        } else {
            quote!(#ty).to_string().split_whitespace().collect()
        }
    }

    fn decode_arg((ident, ty): (&syn::Ident, &syn::Type)) -> proc_macro2::TokenStream {
        if is_env_ty(ty) {
            // napi-rs injects `Env` from the host; it is not a JS argument, so
            // bind a synthetic value and consume no wire arg.
            if matches!(ty, syn::Type::Reference(r) if r.mutability.is_some()) {
                return quote! { let mut #ident = ::napi_oop::Env; };
            }
            return quote! { let #ident = ::napi_oop::Env; };
        }
        if let Some((inputs, _)) = fn_trait_sig(ty) {
            let cb: Vec<_> = (0..inputs.len()).map(|i| format_ident!("__c{i}")).collect();
            quote! { let #ident = { let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap_or(::napi_oop::rmpv::Value::Nil)).map_err(|e| ::std::string::ToString::to_string(&e))?; let __cbh = ::napi_oop::tsfn::CallbackHandle::new(__h, ::std::sync::Arc::clone(__cb)); move |#(#cb: #inputs),*| { __cbh.invoke(::std::vec![#(::napi_oop::wire::to_wire(&#cb).unwrap()),*]); } }; }
        } else if tsfn_inner(ty).is_some() {
            quote! { let #ident = { let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap_or(::napi_oop::rmpv::Value::Nil)).map_err(|e| ::std::string::ToString::to_string(&e))?; ::napi_oop::ThreadsafeFunction::__new(__h, ::std::sync::Arc::clone(__cb)) }; }
        } else if let Some(owned) = external_ref_inner(ty) {
            quote! { let #ident: #owned = ::napi_oop::wire::from_wire(__iter.next().unwrap_or(::napi_oop::rmpv::Value::Nil)).map_err(|e| ::std::string::ToString::to_string(&e))?; }
        } else {
            quote! { let #ident: #ty = ::napi_oop::wire::from_wire(__iter.next().unwrap_or(::napi_oop::rmpv::Value::Nil)).map_err(|e| ::std::string::ToString::to_string(&e))?; }
        }
    }
    fn fn_trait_sig(ty: &syn::Type) -> Option<(Vec<syn::Type>, syn::Type)> {
        let bounds = match ty {
            syn::Type::ImplTrait(it) => &it.bounds,
            _ => return None,
        };
        for bound in bounds {
            if let syn::TypeParamBound::Trait(tb) = bound {
                let seg = tb.path.segments.last()?;
                if !matches!(seg.ident.to_string().as_str(), "Fn" | "FnMut" | "FnOnce") {
                    continue;
                }
                if let syn::PathArguments::Parenthesized(p) = &seg.arguments {
                    let inputs: Vec<syn::Type> = p.inputs.iter().cloned().collect();
                    let output = match &p.output {
                        syn::ReturnType::Type(_, t) => (**t).clone(),
                        syn::ReturnType::Default => syn::parse_quote!(()),
                    };
                    return Some((inputs, output));
                }
            }
        }
        None
    }

    /// If `ty` is `&External<T>` (an immutable reference to an external handle),
    /// return the owned `External<T>` type. Such a param is decoded by value (the
    /// `Deserialize` impl rebuilds the handle from the slab) and passed by
    /// reference at the call site, mirroring napi-rs's `&External<T>` signatures.
    fn external_ref_inner(ty: &syn::Type) -> Option<syn::Type> {
        let syn::Type::Reference(r) = ty else {
            return None;
        };
        if r.mutability.is_some() {
            return None;
        }
        let syn::Type::Path(p) = &*r.elem else {
            return None;
        };
        if p.path.segments.last()?.ident == "External" {
            Some((*r.elem).clone())
        } else {
            None
        }
    }

    /// True if `ty` is napi's `Env` in any form (`Env`, `&Env`, `&mut Env`).
    /// Such a parameter is host-injected in napi-rs (no JS argument), so the
    /// dispatch thunk binds a synthetic `Env` rather than decoding from the wire,
    /// and it is omitted from the manifest's parameter list.
    fn is_env_ty(ty: &syn::Type) -> bool {
        let inner = match ty {
            syn::Type::Reference(r) => &*r.elem,
            other => other,
        };
        if let syn::Type::Path(p) = inner {
            if let Some(seg) = p.path.segments.last() {
                return seg.ident == "Env";
            }
        }
        false
    }

    /// The token used to pass a decoded arg at the call site: by reference for
    /// `&External<T>` params (decoded by value) and for `&Env`/`&mut Env`
    /// (synthesised by value), otherwise the value directly.
    fn call_arg_token(ident: &syn::Ident, ty: &syn::Type) -> proc_macro2::TokenStream {
        if external_ref_inner(ty).is_some() {
            quote!(&#ident)
        } else if is_env_ty(ty) {
            if matches!(ty, syn::Type::Reference(r) if r.mutability.is_some()) {
                quote!(&mut #ident)
            } else if matches!(ty, syn::Type::Reference(_)) {
                quote!(&#ident)
            } else {
                quote!(#ident)
            }
        } else {
            quote!(#ident)
        }
    }

    /// If `ty` is `ThreadsafeFunction<T>`, return `T`. Used to recognise the
    /// explicit callback form alongside the `impl Fn(..)` sugar.
    fn tsfn_inner(ty: &syn::Type) -> Option<syn::Type> {
        let path = match ty {
            syn::Type::Path(p) => &p.path,
            _ => return None,
        };
        let seg = path.segments.last()?;
        if seg.ident != "ThreadsafeFunction" {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
            for arg in &a.args {
                if let syn::GenericArgument::Type(t) = arg {
                    return Some(t.clone());
                }
            }
        }
        None
    }

    /// If the return type is `Result<T, _>` (any path ending in `Result`), return
    /// the `T`. The Err arm maps to an error reply; the manifest types `T`.
    fn result_ok_type(output: &syn::ReturnType) -> Option<syn::Type> {
        let ty = match output {
            syn::ReturnType::Type(_, ty) => ty,
            syn::ReturnType::Default => return None,
        };
        let path = match &**ty {
            syn::Type::Path(p) => &p.path,
            _ => return None,
        };
        let seg = path.segments.last()?;
        if seg.ident != "Result" {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(t)) = a.args.first() {
                return Some(t.clone());
            }
        }
        None
    }

    /// Render a TS function-type string (`(a0: T, …) => void`) for a callback
    /// param. Callbacks are fire-and-forget, so the return is always `void`.
    fn ts_fn_type(inputs: &[syn::Type]) -> String {
        let params: Vec<String> = inputs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let ts: String = quote!(#t).to_string().split_whitespace().collect();
                format!("a{i}:{ts}")
            })
            .collect();
        format!("({})=>()", params.join(","))
    }

    /// Render the TS type for an explicit `ThreadsafeFunction<T>` param. These are
    /// `CalleeHandled` by default, so — exactly like vanilla napi-rs — the JS
    /// callback is invoked error-first as `(err: Error | null, value: T) => void`.
    /// The leading `err` param is encoded with the `__NapiCbErr` sentinel that the
    /// manifest maps to `Error | null`.
    fn ts_tsfn_type(inner: &syn::Type) -> String {
        let value: String = quote!(#inner).to_string().split_whitespace().collect();
        format!("(err:__NapiCbErr,a0:{value})=>()")
    }
}
