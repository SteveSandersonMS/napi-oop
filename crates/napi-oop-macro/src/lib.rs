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

    /// In-process mode. Pass the annotated item through unchanged; a real build
    /// layers napi-rs's `#[napi]` for the in-process binding.
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        item
    }
}

#[cfg(feature = "out-of-proc")]
mod out_of_proc {
    use proc_macro::TokenStream;
    use quote::{format_ident, quote};
    use syn::{parse_macro_input, FnArg, ItemFn, PatType};

    /// Out-of-process mode: keep the original function and additionally emit a
    /// dispatch thunk (decode args via the wire codec, call, encode result) plus
    /// an [`inventory`] registration of it under the function's name.
    pub(super) fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
        let func = parse_macro_input!(item as ItemFn);
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
                        let __sink = ::std::sync::Arc::clone(__cb);
                        move |#(#cb_args: #inputs),*| {
                            __sink.invoke(__h, ::std::vec![
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
        let ret_type_str: String = match &func.sig.output {
            syn::ReturnType::Default => "()".to_string(),
            syn::ReturnType::Type(_, ty) => quote!(#ty).to_string().split_whitespace().collect(),
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
                    ::napi_oop::wire::to_wire(&__ret)
                        .map_err(|e| ::std::string::ToString::to_string(&e))
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

    /// If `ty` is `impl Fn(A, B, …) -> R` (or FnMut/FnOnce), return its input
    /// types and return type. Used to recognise callback params.
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
