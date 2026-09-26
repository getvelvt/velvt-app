//! The JSON Schema validator for `proto/schema/`, shared by two test crates.
//!
//! `shared-types/tests/schema_conformance.rs` uses it to check the schemas
//! against the Rust DTOs with instances generated from the schemas.
//! `rust-service/tests/emitted_payload_schema.rs` includes this file with
//! `#[path]` to validate what the real service emits, which a generated
//! instance cannot show: the schema said `daily_activity` holds exactly 7
//! rows for as long as Rust sent 14, and every generated instance agreed with
//! the schema because it was built from it.
//!
//! It implements exactly the keywords `proto/schema/` uses; the keyword check
//! in `schema_conformance.rs` fails any schema that starts using another.

use serde_json::Value;

pub fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference
                .strip_prefix('#')
                .unwrap_or_else(|| panic!("only local $ref is supported, found {reference}"));
            root.pointer(pointer)
                .unwrap_or_else(|| panic!("$ref {reference} does not resolve"))
        }
        None => schema,
    }
}

pub fn types_of(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Validates `value` against `schema` for exactly the keywords listed in
/// `schema_conformance.rs`'s `UNDERSTOOD_KEYWORDS`, which fails any schema
/// that uses another one. `root` is the whole schema file, for local `$ref`s.
pub fn validate(root: &Value, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let schema = resolve(root, schema);
    if let Some(constant) = schema.get("const") {
        if value != constant {
            return Err(format!("{at}: expected const {constant}, found {value}"));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{at}: {value} is not one of {options:?}"));
        }
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matching = branches
            .iter()
            .filter(|branch| validate(root, branch, value, at).is_ok())
            .count();
        if matching != 1 {
            return Err(format!(
                "{at}: {value} matches {matching} of the oneOf branches, not exactly one"
            ));
        }
    }
    let types = types_of(schema);
    if !types.is_empty() && !types.iter().any(|kind| has_type(value, kind)) {
        return Err(format!("{at}: {value} is not of type {types:?}"));
    }
    match value {
        Value::String(text) => {
            let length = text.chars().count() as u64;
            if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
                if length < minimum {
                    return Err(format!("{at}: shorter than minLength {minimum}"));
                }
            }
            if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64) {
                if length > maximum {
                    return Err(format!("{at}: longer than maxLength {maximum}"));
                }
            }
            match schema.get("format").and_then(Value::as_str) {
                Some("uuid") if uuid::Uuid::parse_str(text).is_err() => {
                    return Err(format!("{at}: {text:?} is not a uuid"));
                }
                Some("date-time") if chrono::DateTime::parse_from_rfc3339(text).is_err() => {
                    return Err(format!("{at}: {text:?} is not an RFC 3339 date-time"));
                }
                Some("date") if chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_err() => {
                    return Err(format!("{at}: {text:?} is not a date"));
                }
                _ => {}
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
                if !matches_known_pattern(pattern, text) {
                    return Err(format!("{at}: {text:?} does not match {pattern}"));
                }
            }
        }
        Value::Number(number) => {
            let numeric = number.as_f64().unwrap();
            if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
                if numeric < minimum {
                    return Err(format!("{at}: {numeric} is below minimum {minimum}"));
                }
            }
            if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
                if numeric > maximum {
                    return Err(format!("{at}: {numeric} is above maximum {maximum}"));
                }
            }
        }
        Value::Array(items) => {
            if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
                if items.len() as u64 > maximum {
                    return Err(format!("{at}: more than maxItems {maximum}"));
                }
            }
            if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
                if (items.len() as u64) < minimum {
                    return Err(format!("{at}: fewer than minItems {minimum}"));
                }
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate(root, item_schema, item, &format!("{at}[{index}]"))?;
                }
            }
        }
        Value::Object(object) => {
            if let Some(maximum) = schema.get("maxProperties").and_then(Value::as_u64) {
                if object.len() as u64 > maximum {
                    return Err(format!("{at}: more than maxProperties {maximum}"));
                }
            }
            for key in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !object.contains_key(key) {
                    return Err(format!("{at}: missing required property `{key}`"));
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            for (key, child) in object {
                match properties.and_then(|declared| declared.get(key)) {
                    Some(child_schema) => {
                        validate(root, child_schema, child, &format!("{at}.{key}"))?;
                    }
                    None => match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            return Err(format!(
                                "{at}: property `{key}` is not declared by the schema"
                            ));
                        }
                        Some(additional @ Value::Object(_)) => {
                            validate(root, additional, child, &format!("{at}.{key}"))?;
                        }
                        _ => {}
                    },
                }
            }
        }
        Value::Bool(_) | Value::Null => {}
    }
    Ok(())
}

fn has_type(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        other => panic!("unsupported type {other}"),
    }
}

/// The three patterns `proto/schema/` uses, checked without a regex crate.
fn matches_known_pattern(pattern: &str, text: &str) -> bool {
    match pattern {
        r"^\d{4}-\d{2}-\d{2}$" => {
            let bytes = text.as_bytes();
            bytes.len() == 10
                && bytes.iter().enumerate().all(|(index, byte)| match index {
                    4 | 7 => *byte == b'-',
                    _ => byte.is_ascii_digit(),
                })
        }
        r"^[^\r\n]*$" => !text.contains(['\r', '\n']),
        other => panic!("pattern {other} is not implemented by this test"),
    }
}
