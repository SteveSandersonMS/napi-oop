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

use crate::registry::{RegisteredFn, RegisteredMethod, RegisteredObject};

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

/// One class method's signature, TS types mapped. `constructor` is the ctor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MethodSignature {
    pub js_name: String,
    pub rust_name: String,
    pub param_names: Vec<String>,
    pub params: Vec<String>,
    pub ret: String,
    pub is_async: bool,
    pub is_getter: bool,
}

/// One `#[napi]` class: its name and methods (incl. the constructor). The TS
/// generator emits a proxy class whose instances hold the provider-side handle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassSignature {
    pub name: String,
    pub methods: Vec<MethodSignature>,
}

/// One `#[napi(object)]` value struct, with field TS types mapped. The generator
/// emits a TS `interface` of this shape so callers get real types, not `unknown`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectSignature {
    /// TS interface name (the Rust struct name, verbatim).
    pub name: String,
    /// Field names (camelCase, as napi-rs / serde rename_all expose them).
    pub field_names: Vec<String>,
    /// Field TypeScript types, aligned with `field_names`.
    pub field_types: Vec<String>,
}

/// The full set of exposed functions, ready to serialize for the TS generator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub functions: Vec<FnSignature>,
    pub classes: Vec<ClassSignature>,
    pub objects: Vec<ObjectSignature>,
}

/// Map a (whitespace-stripped) Rust type to its TypeScript equivalent. Falls back
/// to `unknown` for types not yet modelled (generalized in the type-system phase).
pub fn rust_to_ts(rust: &str) -> String {
    rust_to_ts_with(rust, &std::collections::HashSet::new())
}

/// As [`rust_to_ts`], but `known` names (class proxies and `#[napi(object)]`
/// interfaces) pass through verbatim instead of degrading to `unknown`, so a
/// struct/class used as a param, return, or container element keeps its TS type.
pub fn rust_to_ts_with(rust: &str, known: &std::collections::HashSet<String>) -> String {
    // A by-reference param (`&External<T>`, `&str`) maps identically to its
    // owned form on the wire, so drop a leading `&` (and `mut`) before mapping.
    let rust = rust
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .trim();
    // Normalize away module paths on the outer type (`napi::Buffer` -> `Buffer`,
    // `napi_oop::External<i32>` -> `External<i32>`) so `#[napi]` source compiles
    // regardless of how the type was imported.
    let rust = strip_outer_path(rust);
    if known.contains(rust) {
        return rust.to_string();
    }
    match rust {
        "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "f32" | "f64" | "i64" | "u64" | "usize"
        | "isize" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "&str" | "str" => "string".to_string(),
        "()" => "void".to_string(),
        "Buffer" => "Uint8Array".to_string(),
        "BigInt" => "bigint".to_string(),
        other => {
            if let Some(ts) = fn_type_to_ts(other) {
                ts
            } else if let Some(inner) = strip_generic(other, "Vec") {
                format!("Array<{}>", rust_to_ts_with(inner, known))
            } else if let Some(inner) = strip_generic(other, "Option") {
                // Matches napi-rs: an `Option<T>` accepts `undefined` or `null`
                // (both decode provider-side as `None`).
                format!("{} | undefined | null", rust_to_ts_with(inner, known))
            } else if strip_generic(other, "External").is_some() {
                // Opaque JS-held handle; backed by a provider-side token.
                "ExternalObject".to_string()
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

/// Strip a module path from the outer type name only, leaving generics intact:
/// `napi::Buffer` -> `Buffer`, `napi_oop::External<i32>` -> `External<i32>`.
fn strip_outer_path(ty: &str) -> &str {
    let head_end = ty.find('<').unwrap_or(ty.len());
    match ty[..head_end].rfind("::") {
        Some(pos) => &ty[pos + 2..],
        None => ty,
    }
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
    let class_names: std::collections::HashSet<&str> = inventory::iter::<RegisteredMethod>
        .into_iter()
        .map(|m| m.class)
        .collect();
    // Names that map to a TS type verbatim (class proxies + object interfaces),
    // so they survive param/return/container mapping instead of becoming `unknown`.
    let mut known: std::collections::HashSet<String> =
        class_names.iter().map(|n| n.to_string()).collect();
    for o in inventory::iter::<RegisteredObject> {
        known.insert(o.name.to_string());
    }
    let functions = inventory::iter::<RegisteredFn>
        .into_iter()
        .filter(|f| !f.name.contains('.')) // class methods are grouped below
        .map(|f| FnSignature {
            js_name: snake_to_camel(f.name),
            rust_name: f.name.to_string(),
            param_names: f.param_names.iter().map(|n| snake_to_camel(n)).collect(),
            params: f
                .params
                .iter()
                .map(|t| rust_to_ts_with(t, &known))
                .collect(),
            ret: rust_to_ts_with(f.ret, &known),
            is_async: f.is_async,
        })
        .collect();
    let mut classes: Vec<ClassSignature> = Vec::new();
    for m in inventory::iter::<RegisteredMethod> {
        let method = MethodSignature {
            js_name: m.method.to_string(),
            rust_name: m.rust_name.to_string(),
            param_names: m.param_names.iter().map(|n| snake_to_camel(n)).collect(),
            params: m
                .params
                .iter()
                .map(|t| rust_to_ts_with(t, &known))
                .collect(),
            ret: rust_to_ts_with(m.ret, &known),
            is_async: m.is_async,
            is_getter: m.is_getter,
        };
        match classes.iter_mut().find(|c| c.name == m.class) {
            Some(c) => c.methods.push(method),
            None => classes.push(ClassSignature {
                name: m.class.to_string(),
                methods: vec![method],
            }),
        }
    }
    let objects = inventory::iter::<RegisteredObject>
        .into_iter()
        .map(|o| ObjectSignature {
            name: o.name.to_string(),
            field_names: o.field_names.iter().map(|n| snake_to_camel(n)).collect(),
            field_types: o
                .field_types
                .iter()
                .map(|t| rust_to_ts_with(t, &known))
                .collect(),
        })
        .collect();
    Manifest {
        functions,
        classes,
        objects,
    }
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
        assert_eq!(rust_to_ts("Option<String>"), "string | undefined | null");
        assert_eq!(rust_to_ts("Vec<Option<u8>>"), "Array<number | undefined | null>");
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
