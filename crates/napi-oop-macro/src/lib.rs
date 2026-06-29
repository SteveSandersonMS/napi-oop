//! The `#[napi]` attribute macro for `napi-oop`.
//!
//! The same annotated source builds two ways, selected by cargo feature:
//!
//! - **`in-proc`** (default): behave like a normal in-process napi-rs build.
//!   For now the item is passed through unchanged (a real build pairs this crate
//!   with napi-rs's own `#[napi]`); a later phase delegates explicitly.
//! - **`out-of-proc`**: emit out-of-process remoting glue — a serde/wire-codec
//!   dispatch thunk plus a registry entry the runtime advertises to Node.
//!
//! The user's source is identical in both modes; only the generated glue differs.

use proc_macro::TokenStream;

/// Drop-in replacement for napi-rs's `#[napi]`. See the crate docs for the
/// in-proc vs out-of-proc build modes.
#[proc_macro_attribute]
pub fn napi(_attr: TokenStream, item: TokenStream) -> TokenStream {
    #[cfg(feature = "out-of-proc")]
    {
        out_of_proc::expand(_attr, item)
    }
    #[cfg(not(feature = "out-of-proc"))]
    {
        in_proc::expand(_attr, item)
    }
}

#[cfg(not(feature = "out-of-proc"))]
mod in_proc {
    use proc_macro::TokenStream;

    /// In-process mode. Pass through; the facade re-exports napi-rs's real
    /// `#[napi]` so source produces a native `.node`.
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        item
    }
}

#[cfg(feature = "out-of-proc")]
mod out_of_proc {
    use proc_macro::TokenStream;
    use quote::{format_ident, quote, ToTokens};
    use syn::{parse_macro_input, FnArg, ImplItem, Item, ItemFn, ItemImpl, PatType};

    /// Out-of-process mode. Free fns get a dispatch thunk + registration; class
    /// `impl` blocks get a thunk per method/constructor (each keyed `Class.method`)
    /// plus class metadata; plain structs/objects pass through (serde carries them).
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        let parsed = parse_macro_input!(item as Item);
        match parsed {
            Item::Fn(func) => expand_fn(func),
            Item::Impl(imp) => expand_impl(imp),
            other => quote!(#other).into(),
        }
    }

    fn expand_fn(func: ItemFn) -> TokenStream {
        let fn_name = func.sig.ident.clone();
        let fn_name_str = fn_name.to_string();

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
        let decode_args = arg_idents.iter().zip(arg_types.iter()).map(|(ident, ty)| {
            if let Some((inputs, _output)) = fn_trait_sig(ty) {
                // `impl Fn(..)` sugar: fire-and-forget closure firing at the peer.
                let cb_args: Vec<_> = (0..inputs.len()).map(|i| format_ident!("__c{i}")).collect();
                quote! {
                    let #ident = {
                        let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap())
                            .map_err(|e| ::std::string::ToString::to_string(&e))?;
                        let __cbh = ::napi_oop::tsfn::CallbackHandle::new(__h, ::std::sync::Arc::clone(__cb));
                        move |#(#cb_args: #inputs),*| {
                            __cbh.invoke(::std::vec![
                                #(::napi_oop::wire::to_wire(&#cb_args).unwrap()),*
                            ]);
                        }
                    };
                }
            } else if tsfn_inner(ty).is_some() {
                // Explicit `ThreadsafeFunction<T>`: decode the handle, hand it the
                // shared sink so it can be stored and fired after the call.
                quote! {
                    let #ident = {
                        let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap())
                            .map_err(|e| ::std::string::ToString::to_string(&e))?;
                        ::napi_oop::ThreadsafeFunction::__new(__h, ::std::sync::Arc::clone(__cb))
                    };
                }
            } else {
                quote! {
                    let #ident: #ty = ::napi_oop::wire::from_wire(__iter.next().unwrap())
                        .map_err(|e| ::std::string::ToString::to_string(&e))?;
                }
            }
        });

        // Stringify each Rust type for the manifest the TS generator consumes;
        // callback params (both forms) become a TS function-type string.
        let param_type_strs: Vec<String> = arg_types
            .iter()
            .map(|ty| {
                if let Some((inputs, _)) = fn_trait_sig(ty) {
                    ts_fn_type(&inputs)
                } else if let Some(inner) = tsfn_inner(ty) {
                    ts_fn_type(std::slice::from_ref(&inner))
                } else {
                    quote!(#ty).to_string().split_whitespace().collect()
                }
            })
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
        let call_expr = if is_async {
            quote! { ::napi_oop::block_on(#fn_name(#(#arg_idents),*)) }
        } else {
            quote! { #fn_name(#(#arg_idents),*) }
        };

        // A `Result<T, E>` Err maps to an error reply (mirroring napi-rs's throw);
        // a plain return is always success. The Ok value / plain value is encoded.
        let encode_ret = if ret_ok_type.is_some() {
            quote! {
                match __ret {
                    ::core::result::Result::Ok(__v) => ::napi_oop::wire::to_wire(&__v)
                        .map_err(|e| ::std::string::ToString::to_string(&e)),
                    ::core::result::Result::Err(__e) => {
                        ::core::result::Result::Err(::std::string::ToString::to_string(&__e))
                    }
                }
            }
        } else {
            quote! {
                ::napi_oop::wire::to_wire(&__ret)
                    .map_err(|e| ::std::string::ToString::to_string(&e))
            }
        };

        let expanded = quote! {
            #func

            const _: () = {
                fn __napi_oop_dispatch(
                    __args: ::std::vec::Vec<::napi_oop::rmpv::Value>,
                    __cb: &::std::sync::Arc<dyn ::napi_oop::registry::Callbacks>,
                ) -> ::core::result::Result<::napi_oop::rmpv::Value, ::std::string::String> {
                    if __args.len() != #arity {
                        return ::core::result::Result::Err(::std::format!(
                            "{} expected {} argument(s), got {}",
                            #fn_name_str,
                            #arity,
                            __args.len(),
                        ));
                    }
                    let mut __iter = __args.into_iter();
                    #(#decode_args)*
                    let __ret = #call_expr;
                    #encode_ret
                }

                ::napi_oop::inventory::submit! {
                    ::napi_oop::registry::RegisteredFn {
                        name: #fn_name_str,
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

    /// A `#[napi] impl Class { … }`: one dispatch thunk per `#[napi]` method,
    /// keyed `Class.method`. The constructor builds the value into the object
    /// slab and returns its top-level handle; methods take the handle as the
    /// first wire arg, borrow the value, and call. Class metadata is registered
    /// so the generator emits a TS class proxy.
    fn expand_impl(imp: ItemImpl) -> TokenStream {
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
            let is_ctor = method
                .attrs
                .iter()
                .any(|a| a.path().is_ident("napi") && a.meta.to_token_stream().to_string().contains("constructor"));
            let is_getter = method
                .attrs
                .iter()
                .any(|a| a.path().is_ident("napi") && a.meta.to_token_stream().to_string().contains("getter"));
            thunks.push(method_thunk(&class_name, self_ty, method, is_ctor, is_getter));
        }
        // Re-emit the impl with inner `#[napi]` attrs stripped (they are not real
        // outer-attribute macros; we generate all glue from this single pass).
        let mut clean = imp.clone();
        for item in &mut clean.items {
            if let ImplItem::Fn(m) = item {
                m.attrs.retain(|a| !a.path().is_ident("napi"));
            }
        }
        // A class instance serializes by minting a slab token, so a fn (free or
        // method) returning the class surfaces as an external handle uniformly.
        // Requires `Clone` (a finite by-value mint of an instance with no GC
        // double-free); instance methods mint directly to avoid relocking.
        let serialize_impl = quote! {
            impl ::napi_oop::serde::Serialize for #self_ty {
                fn serialize<__S: ::napi_oop::serde::Serializer>(&self, __s: __S) -> ::core::result::Result<__S::Ok, __S::Error> {
                    use ::napi_oop::serde::ser::SerializeMap;
                    let __tok = ::napi_oop::types::object_new(::std::boxed::Box::new(::std::clone::Clone::clone(self)));
                    let mut __m = __s.serialize_map(::core::option::Option::Some(1))?;
                    __m.serialize_entry(::napi_oop::types::EXTERNAL_KEY, &__tok)?;
                    __m.end()
                }
            }
        };
        quote! { #clean #serialize_impl #(#thunks)* }.into()
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
        let js_method = if is_ctor { "constructor".to_string() } else { camel(&m_ident.to_string()) };
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
        let param_strs: Vec<String> = arg_types.iter().map(ts_param).collect();
        let is_async = method.sig.asyncness.is_some();
        let ret_ok = result_ok_type(&method.sig.output);
        let ret_str: String = match (&ret_ok, &method.sig.output) {
            (Some(ok), _) => quote!(#ok).to_string().split_whitespace().collect(),
            (None, syn::ReturnType::Type(_, ty)) => quote!(#ty).to_string().split_whitespace().collect(),
            (None, syn::ReturnType::Default) => "()".to_string(),
        };

        let mut ret_label = ret_str.clone();
        let ret_is_class = ret_str == class || ret_str == "Self";
        if ret_is_class {
            ret_label = class.to_string();
        }
        // Receiver count: a constructor has none; methods take handle first.
        let takes_self = matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)));
        let m_ident2 = m_ident.clone();
        let body = if is_ctor {
            let call = quote!(#self_ty::#m_ident2(#(#arg_idents),*));
            let wrap = if ret_ok.is_some() { quote!(match __ret { Ok(v)=>v, Err(e)=>return Err(::std::string::ToString::to_string(&e)) }) } else { quote!(__ret) };
            quote! {
                let mut __iter = __args.into_iter();
                #(#decode)*
                let __ret = #call;
                let __obj = #wrap;
                let __tok = ::napi_oop::types::object_new(::std::boxed::Box::new(__obj));
                ::core::result::Result::Ok(::napi_oop::wire::external_marker(__tok))
            }
        } else if takes_self {
            let call = if is_async { quote!(::napi_oop::block_on(__self.#m_ident2(#(#arg_idents),*))) } else { quote!(__self.#m_ident2(#(#arg_idents),*)) };
            if ret_is_class {
                // Mint the returned instance AFTER releasing the slab lock; minting
                // inside `with_object` would re-enter the slab mutex and deadlock.
                let unwrap = if ret_ok.is_some() { quote!(match __r { Ok(v)=>v, Err(e)=>return Err(::std::string::ToString::to_string(&e)) }) } else { quote!(__r) };
                quote! {
                    let mut __iter = __args.into_iter();
                    let __tok = ::napi_oop::wire::external_handle(&__iter.next().ok_or("missing receiver handle")?)?;
                    #(#decode)*
                    let __r = ::napi_oop::types::with_object::<#self_ty,_>(__tok, |__self| #call).ok_or("object handle no longer live")?;
                    let __obj = #unwrap;
                    let __new = ::napi_oop::types::object_new(::std::boxed::Box::new(__obj));
                    ::core::result::Result::Ok(::napi_oop::wire::external_marker(__new))
                }
            } else {
                let encode = encode_ret(&ret_ok);
                quote! {
                    let mut __iter = __args.into_iter();
                    let __tok = ::napi_oop::wire::external_handle(&__iter.next().ok_or("missing receiver handle")?)?;
                    #(#decode)*
                    ::napi_oop::types::with_object::<#self_ty,_>(__tok, |__self| { let __ret = #call; #encode }).ok_or("object handle no longer live")?
                }
            }
        } else {
            // associated fn (static) — treat like a free fn
            let call = if is_async { quote!(::napi_oop::block_on(#self_ty::#m_ident2(#(#arg_idents),*))) } else { quote!(#self_ty::#m_ident2(#(#arg_idents),*)) };
            let encode = method_encode(&ret_ok, ret_is_class);
            quote! { let mut __iter = __args.into_iter(); #(#decode)* let __ret = #call; #encode }
        };

        quote! {
            const _: () = {
                fn #dispatch_ident(__args: ::std::vec::Vec<::napi_oop::rmpv::Value>, __cb: &::std::sync::Arc<dyn ::napi_oop::registry::Callbacks>) -> ::core::result::Result<::napi_oop::rmpv::Value, ::std::string::String> {
                    #body
                }
                ::napi_oop::inventory::submit! { ::napi_oop::registry::RegisteredFn { name: #wire_name, dispatch: #dispatch_ident, params: &[#(#param_strs),*], param_names: &[#(#arg_names),*], ret: #ret_label, is_async: #is_async } }
                ::napi_oop::inventory::submit! { ::napi_oop::registry::RegisteredMethod { class: #class, method: #js_method, rust_name: #wire_name, params: &[#(#param_strs),*], param_names: &[#(#arg_names),*], ret: #ret_label, is_async: #is_async, is_getter: #is_getter } }
            };
        }
    }

    fn camel(name: &str) -> String {
        let mut out = String::new();
        let mut up = false;
        for c in name.chars() {
            if c == '_' { up = true; } else if up { out.extend(c.to_uppercase()); up = false; } else { out.push(c); }
        }
        out
    }

    fn encode_ret(ret_ok: &Option<syn::Type>) -> proc_macro2::TokenStream {
        if ret_ok.is_some() {
            quote! { match __ret { Ok(v)=>::napi_oop::wire::to_wire(&v).map_err(|e| ::std::string::ToString::to_string(&e)), Err(e)=>Err(::std::string::ToString::to_string(&e)) } }
        } else {
            quote! { ::napi_oop::wire::to_wire(&__ret).map_err(|e| ::std::string::ToString::to_string(&e)) }
        }
    }

    /// Encode a method/static return. When the return is a class instance it is
    /// minted into the object slab and surfaced as an external handle token.
    fn method_encode(ret_ok: &Option<syn::Type>, ret_is_class: bool) -> proc_macro2::TokenStream {
        if !ret_is_class {
            return encode_ret(ret_ok);
        }
        if ret_ok.is_some() {
            quote! { match __ret { Ok(v)=>{ let __tok = ::napi_oop::types::object_new(::std::boxed::Box::new(v)); Ok(::napi_oop::wire::external_marker(__tok)) }, Err(e)=>Err(::std::string::ToString::to_string(&e)) } }
        } else {
            quote! { { let __tok = ::napi_oop::types::object_new(::std::boxed::Box::new(__ret)); Ok(::napi_oop::wire::external_marker(__tok)) } }
        }
    }

    fn ts_param(ty: &syn::Type) -> String {
        if let Some((inputs, _)) = fn_trait_sig(ty) { ts_fn_type(&inputs) }
        else if let Some(inner) = tsfn_inner(ty) { ts_fn_type(std::slice::from_ref(&inner)) }
        else { quote!(#ty).to_string().split_whitespace().collect() }
    }

    fn decode_arg((ident, ty): (&syn::Ident, &syn::Type)) -> proc_macro2::TokenStream {
        if let Some((inputs, _)) = fn_trait_sig(ty) {
            let cb: Vec<_> = (0..inputs.len()).map(|i| format_ident!("__c{i}")).collect();
            quote! { let #ident = { let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap()).map_err(|e| ::std::string::ToString::to_string(&e))?; let __cbh = ::napi_oop::tsfn::CallbackHandle::new(__h, ::std::sync::Arc::clone(__cb)); move |#(#cb: #inputs),*| { __cbh.invoke(::std::vec![#(::napi_oop::wire::to_wire(&#cb).unwrap()),*]); } }; }
        } else if tsfn_inner(ty).is_some() {
            quote! { let #ident = { let __h = ::napi_oop::wire::callback_handle(&__iter.next().unwrap()).map_err(|e| ::std::string::ToString::to_string(&e))?; ::napi_oop::ThreadsafeFunction::__new(__h, ::std::sync::Arc::clone(__cb)) }; }
        } else {
            quote! { let #ident: #ty = ::napi_oop::wire::from_wire(__iter.next().unwrap()).map_err(|e| ::std::string::ToString::to_string(&e))?; }
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
}
