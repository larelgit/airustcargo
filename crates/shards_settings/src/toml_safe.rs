//! Whether a settings group survives being written as TOML.
//!
//! Bevy's settings framework writes a group with `toml::Value::try_from(..).unwrap()`: a
//! group that TOML cannot hold does not fail, it panics, at the moment the player changes a
//! setting. TOML integers are signed 64-bit and TOML table keys are strings, so a `u64`
//! field (any value above `i64::MAX` panics) or a map keyed by something else is a trap.
//! [`check_toml_safe`] walks the type once, at registration, and names the field.

use std::any::TypeId;

use bevy::reflect::enums::VariantInfo;
use bevy::reflect::{TypeInfo, Typed};

/// Walks the reflected type of `G` and returns the first field TOML cannot hold safely, as
/// a message that names it (`MyGroup.score: u64 ...`).
pub fn check_toml_safe<G: Typed>() -> Result<(), String> {
    let info = G::type_info();
    let name = info.type_path_table().ident().unwrap_or("group");
    walk(info, name, &mut Vec::new())
}

fn walk(info: &'static TypeInfo, path: &str, seen: &mut Vec<TypeId>) -> Result<(), String> {
    // A type that contains itself (a tree) is checked once.
    if seen.contains(&info.type_id()) {
        return Ok(());
    }
    seen.push(info.type_id());
    let result = match info {
        TypeInfo::Struct(s) => s.iter().try_for_each(|f| field(f.type_info(), f.type_path(), &format!("{path}.{}", f.name()), seen)),
        TypeInfo::TupleStruct(t) => t.iter().try_for_each(|f| field(f.type_info(), f.type_path(), &format!("{path}.{}", f.index()), seen)),
        TypeInfo::Tuple(t) => t.iter().try_for_each(|f| field(f.type_info(), f.type_path(), &format!("{path}.{}", f.index()), seen)),
        TypeInfo::List(l) => field(l.item_info(), l.item_ty().path(), &format!("{path}[]"), seen),
        TypeInfo::Array(a) => field(a.item_info(), a.item_ty().path(), &format!("{path}[]"), seen),
        TypeInfo::Set(s) => field(None, s.value_ty().path(), &format!("{path}[]"), seen),
        TypeInfo::Map(m) => {
            if !m.key_ty().is::<String>() {
                Err(format!(
                    "{path}: a map keyed by `{}` cannot be written as TOML (table keys are strings); use `BTreeMap<String, _>`",
                    m.key_ty().path()
                ))
            } else {
                field(m.value_info(), m.value_ty().path(), &format!("{path}{{}}"), seen)
            }
        }
        TypeInfo::Enum(e) => e.iter().try_for_each(|variant| match variant {
            VariantInfo::Unit(_) => Ok(()),
            VariantInfo::Tuple(t) => {
                t.iter().try_for_each(|f| field(f.type_info(), f.type_path(), &format!("{path}::{}.{}", variant.name(), f.index()), seen))
            }
            VariantInfo::Struct(s) => {
                s.iter().try_for_each(|f| field(f.type_info(), f.type_path(), &format!("{path}::{}.{}", variant.name(), f.name()), seen))
            }
        }),
        TypeInfo::Opaque(_) => scalar(info.type_path(), path),
    };
    seen.pop();
    result
}

/// A field: its static type information when it has some, else judged by its path alone.
fn field(info: Option<&'static TypeInfo>, type_path: &str, path: &str, seen: &mut Vec<TypeId>) -> Result<(), String> {
    match info {
        Some(info) => walk(info, path, seen),
        None => scalar(type_path, path),
    }
}

/// Opaque types: the ones TOML holds, and the ones it cannot.
fn scalar(type_path: &str, path: &str) -> Result<(), String> {
    match type_path {
        "bool" | "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "f32" | "f64" | "char" | "alloc::string::String" => Ok(()),
        "u64" | "usize" | "u128" | "i128" => Err(format!(
            "{path}: {type_path} is not TOML-safe (TOML integers are signed 64-bit, so a large value would panic when saved); \
             use i64, u32, f64 or a String"
        )),
        other => Err(format!(
            "{path}: `{other}` is not a type a settings file holds; use bool, numbers up to 32 bits, f32/f64, String, enums, \
             Vec and BTreeMap<String, _> of those"
        )),
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;

    #[derive(Reflect, Default)]
    struct Plain {
        _flag: bool,
        _count: i32,
        _ratio: f32,
        _name: String,
        _list: Vec<String>,
        _map: std::collections::BTreeMap<String, Vec<String>>,
        _mode: Mode,
    }

    #[derive(Reflect, Default)]
    enum Mode {
        #[default]
        A,
        _B(u32),
        _C { _x: f64 },
    }

    #[derive(Reflect, Default)]
    struct BigCounter {
        _fine: i64,
        score: u64,
    }

    #[derive(Reflect, Default)]
    struct BadKey {
        _by_id: std::collections::BTreeMap<u32, String>,
    }

    #[derive(Reflect, Default)]
    struct Nested {
        _inner: Vec<BigCounter>,
    }

    #[derive(Reflect)]
    struct Tree {
        _children: Vec<Tree>,
        _leaf: i32,
    }

    #[test]
    fn plain_groups_pass() {
        assert_eq!(check_toml_safe::<Plain>(), Ok(()));
        assert_eq!(check_toml_safe::<Tree>(), Ok(()), "a type that contains itself ends");
        let _ = Mode::A;
    }

    #[test]
    fn the_message_names_the_field() {
        let err = check_toml_safe::<BigCounter>().unwrap_err();
        assert!(err.contains("BigCounter.score") && err.contains("u64") && err.contains("i64"), "{err}");
        let err = check_toml_safe::<BadKey>().unwrap_err();
        assert!(err.contains("BadKey._by_id") && err.contains("u32") && err.contains("BTreeMap<String"), "{err}");
        let err = check_toml_safe::<Nested>().unwrap_err();
        assert!(err.contains("Nested._inner[].score"), "{err}");
    }
}
