//! Type manifest: the bridge from the Rust `#[napi]` signatures (the IDL) to the
//! TypeScript generator.
//!
//! The macro records each function's parameter and return Rust types as strings
//! in its [`crate::registry::RegisteredFn`]. This module maps those Rust types to
//! TypeScript types and exports the whole registry as a JSON manifest. A Node
//! generator reads the manifest and emits a typed binding (`index.d.ts` /
//! `index.js`), exactly as napi-rs emits one for the in-process build — so the
//! caller's TS is generated, never hand-written.
//!
//! Async vs sync: the generator decides how to wrap the return type (a `Promise`
//! for the async binding, a bare value for the sync one), so the manifest stores
//! the *unwrapped* return type.

use serde::Serialize;

use crate::registry::RegisteredFn;

/// One function's signature, with TypeScript types already mapped. The JS name is
/// the camelCase form napi-rs would expose.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FnSignature {
    /// JS-facing name (`add_numbers` -> `addNumbers`).
    pub js_name: String,
    /// Wire name (the Rust function name) used by the dispatcher.
    pub rust_name: String,
    /// Parameter names, in order (camelCase, as napi-rs would expose).
    pub param_names: Vec<String>,
    /// Parameter TypeScript types, in order.
    pub params: Vec<String>,
    /// Return TypeScript type (unwrapped; the generator adds `Promise<>` if async).
    pub ret: String,
    /// Whether the Rust fn is async — surfaced as `Promise<T>` on TS in both
    /// binding modes.
    pub is_async: bool,
}

/// The full set of exposed functions, ready to serialize for the TS generator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub functions: Vec<FnSignature>,
}

/// Map a (whitespace-stripped) Rust type to its TypeScript equivalent. Falls back
/// to `unknown` for types not yet modelled (generalized in the type-system phase).
pub fn rust_to_ts(rust: &str) -> String {
    match rust {
        "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "f32" | "f64" | "i64" | "u64" | "usize"
        | "isize" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "&str" | "str" => "string".to_string(),
        "()" => "void".to_string(),
        other => {
            if let Some(ts) = fn_type_to_ts(other) {
                ts
            } else if let Some(inner) = strip_generic(other, "Vec") {
                format!("Array<{}>", rust_to_ts(inner))
            } else if let Some(inner) = strip_generic(other, "Option") {
                format!("{} | null", rust_to_ts(inner))
            } else {
                "unknown".to_string()
            }
        }
    }
}

/// `Vec<i64>` -> `Some("i64")` for wrapper `"Vec"`; otherwise `None`.
fn strip_generic<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let rest = ty.strip_prefix(wrapper)?.strip_prefix('<')?;
    rest.strip_suffix('>')
}

/// Map a callback param the macro encoded as `(a0:i32,a1:i32)=>i32` into a TS
/// function type, mapping each param/return Rust type. `None` if not a fn type.
fn fn_type_to_ts(ty: &str) -> Option<String> {
    let (params, ret) = ty.strip_prefix('(')?.split_once(")=>")?;
    let mapped: Vec<String> = if params.is_empty() {
        Vec::new()
    } else {
        params
            .split(',')
            .map(|p| {
                let (name, t) = p.split_once(':').unwrap_or(("a", p));
                format!("{name}:{}", rust_to_ts(t))
            })
            .collect()
    };
    Some(format!("({})=>{}", mapped.join(","), rust_to_ts(ret)))
}

/// Convert a snake_case Rust name to camelCase, mirroring napi-rs.
fn snake_to_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Build the manifest from all registered `#[napi]` functions.
pub fn manifest() -> Manifest {
    let functions = inventory::iter::<RegisteredFn>
        .into_iter()
        .map(|f| FnSignature {
            js_name: snake_to_camel(f.name),
            rust_name: f.name.to_string(),
            param_names: f.param_names.iter().map(|n| snake_to_camel(n)).collect(),
            params: f.params.iter().map(|t| rust_to_ts(t)).collect(),
            ret: rust_to_ts(f.ret),
            is_async: f.is_async,
        })
        .collect();
    Manifest { functions }
}

/// Serialize the manifest to pretty JSON for the TS generator to consume.
pub fn manifest_json() -> String {
    serde_json::to_string_pretty(&manifest()).expect("manifest serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_primitives() {
        assert_eq!(rust_to_ts("i64"), "number");
        assert_eq!(rust_to_ts("bool"), "boolean");
        assert_eq!(rust_to_ts("String"), "string");
        assert_eq!(rust_to_ts("()"), "void");
    }

    #[test]
    fn maps_containers() {
        assert_eq!(rust_to_ts("Vec<i64>"), "Array<number>");
        assert_eq!(rust_to_ts("Option<String>"), "string | null");
        assert_eq!(rust_to_ts("Vec<Option<u8>>"), "Array<number | null>");
    }

    #[test]
    fn unmodelled_is_unknown() {
        assert_eq!(rust_to_ts("MyStruct"), "unknown");
    }

    #[test]
    fn snake_to_camel_matches_napi() {
        assert_eq!(snake_to_camel("add_numbers"), "addNumbers");
        assert_eq!(snake_to_camel("x"), "x");
    }
}
