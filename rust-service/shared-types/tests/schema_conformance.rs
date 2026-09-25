//! `proto/schema/` against the Rust DTOs, file by file.
//!
//! `AGENTS.md` says the JSON Schemas are authoritative and that both
//! workspaces conform to them, and until this file nothing checked it. The
//! schemas drifted without a failing build: `unclassified_triage.json` declared
//! an `entries[].bundle_id` that the Rust type deliberately omits and never
//! sends, and the protocol changelog and the Classification v2 contract both
//! described the field as though it crossed the socket.
//!
//! For every message schema this test builds two instances from the schema
//! itself, with no hand-written fixtures to fall out of date:
//!
//! - **maximal**: every property the schema declares, optional ones included;
//! - **minimal**: only the properties the schema requires.
//!
//! Each must parse as a `ClientMessage` or `ServerMessage`. Most payload
//! structs deny unknown fields, so a property the schema declares and Rust does
//! not have fails the maximal instance, and a field Rust requires that the
//! schema calls optional fails the minimal one. Each parsed message is then
//! serialized again and validated against the same schema, so a field Rust
//! emits that the schema does not declare, or a value of the wrong type, fails
//! too. The validator below implements exactly the keywords `proto/schema/`
//! uses, and refuses a schema that uses any other, so a new keyword cannot be
//! silently skipped.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{json, Map, Value};
use velvt_shared_types::{ClientMessage, ServerMessage};

/// Every keyword the validator and the generator understand. A schema using
/// anything else fails `every_schema_uses_only_understood_keywords` rather
/// than having that keyword ignored.
const UNDERSTOOD_KEYWORDS: &[&str] = &[
    "$schema",
    "$comment",
    "$ref",
    "$defs",
    "title",
    "description",
    "default",
    "type",
    "const",
    "enum",
    "format",
    "pattern",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "properties",
    "required",
    "additionalProperties",
    "maxProperties",
    "items",
    "minItems",
    "maxItems",
    "oneOf",
];

fn schema_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto/schema")
}

fn schemas() -> Vec<(String, Value)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(schema_directory())
        .expect("proto/schema is readable")
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    assert!(
        files.len() > 50,
        "found only {} schema files; the path to proto/schema is wrong",
        files.len()
    );
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            let schema: Value = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("{name} is not JSON: {error}"));
            (name, schema)
        })
        .collect()
}

/// The `type` const of a message schema, or `None` for one of the five
/// schemas that describe only a payload.
fn message_type(schema: &Value) -> Option<&str> {
    schema
        .pointer("/properties/type/const")
        .and_then(Value::as_str)
}

/// A schema file as a whole message: an envelope schema is used as it is, and
/// a payload-only schema is wrapped in the envelope its file name implies, so
/// all of `proto/schema/` is checked rather than the files that happen to
/// spell out their envelope.
fn as_envelope(name: &str, schema: &Value) -> (String, Value) {
    if let Some(kind) = message_type(schema) {
        return (kind.to_owned(), schema.clone());
    }
    let kind = name.trim_end_matches(".json").to_owned();
    let mut payload = schema.clone();
    let defs = payload
        .as_object_mut()
        .and_then(|object| object.remove("$defs"));
    let mut envelope = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["type", "payload"],
        "properties": {
            "type": { "const": kind },
            "payload": payload,
        },
    });
    if let Some(defs) = defs {
        envelope["$defs"] = defs;
    }
    (kind, envelope)
}

#[test]
fn every_schema_uses_only_understood_keywords() {
    for (name, schema) in schemas() {
        let mut unknown = BTreeSet::new();
        collect_unknown_keywords(&schema, &mut unknown);
        assert!(
            unknown.is_empty(),
            "{name} uses {unknown:?}, which this conformance test does not implement. \
             Teach the validator and the generator the keyword before using it"
        );
    }
}

fn collect_unknown_keywords(schema: &Value, unknown: &mut BTreeSet<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    for (keyword, value) in object {
        if !UNDERSTOOD_KEYWORDS.contains(&keyword.as_str()) {
            unknown.insert(keyword.clone());
        }
        match keyword.as_str() {
            "properties" | "$defs" => {
                for child in value.as_object().into_iter().flat_map(Map::values) {
                    collect_unknown_keywords(child, unknown);
                }
            }
            "items" | "additionalProperties" => collect_unknown_keywords(value, unknown),
            "oneOf" => {
                for branch in value.as_array().into_iter().flatten() {
                    collect_unknown_keywords(branch, unknown);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn every_message_schema_matches_the_rust_types_in_both_directions() {
    let mut checked = 0;
    let mut failures = Vec::new();
    for (name, file_schema) in schemas() {
        let (kind, schema) = as_envelope(&name, &file_schema);
        for (shape, instance) in [
            ("maximal", generate(&schema, &schema, Shape::Maximal)),
            ("minimal", generate(&schema, &schema, Shape::Minimal)),
        ] {
            if let Err(problem) = validate(&schema, &schema, &instance, "$") {
                failures.push(format!(
                    "{name}: the {shape} instance generated from the schema does not \
                     validate against it ({problem}); the generator needs teaching"
                ));
                continue;
            }
            let reencoded = match parse_as_message(&instance) {
                Ok(value) => value,
                Err(error) => {
                    failures.push(format!(
                        "{name}: Rust rejects the schema's {shape} `{kind}` message: {error}\n    \
                         instance: {instance}"
                    ));
                    continue;
                }
            };
            if let Err(problem) = validate(&schema, &schema, &reencoded, "$") {
                failures.push(format!(
                    "{name}: Rust re-serializes the {shape} `{kind}` message into something \
                     the schema rejects: {problem}\n    emitted: {reencoded}"
                ));
            }
        }
        checked += 1;
    }
    assert_eq!(
        checked,
        schemas().len(),
        "every schema file is a message or a message payload"
    );
    assert!(
        failures.is_empty(),
        "{} conformance failure(s) between proto/schema and velvt-shared-types:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Parses `instance` as whichever message enum knows its `type`, and returns
/// what Rust serializes it back to.
fn parse_as_message(instance: &Value) -> Result<Value, String> {
    let as_client = serde_json::from_value::<ClientMessage>(instance.clone());
    if let Ok(message) = as_client {
        return Ok(serde_json::to_value(message).unwrap());
    }
    let as_server = serde_json::from_value::<ServerMessage>(instance.clone());
    match (as_client, as_server) {
        (_, Ok(message)) => Ok(serde_json::to_value(message).unwrap()),
        (Err(client), Err(server)) => Err(format!(
            "as ClientMessage: {client}; as ServerMessage: {server}"
        )),
        (Ok(_), _) => unreachable!(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Maximal,
    Minimal,
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
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

fn types_of(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// A value the schema accepts. Maximal fills every declared property; minimal
/// fills only the required ones. Nullable types prefer the non-null branch in
/// both, because a null exercises nothing.
fn generate(root: &Value, schema: &Value, shape: Shape) -> Value {
    let schema = resolve(root, schema);
    if let Some(constant) = schema.get("const") {
        return constant.clone();
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        return options
            .iter()
            .find(|option| !option.is_null())
            .or_else(|| options.first())
            .cloned()
            .unwrap_or(Value::Null);
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let branch = branches
            .iter()
            .find(|branch| types_of(resolve(root, branch)) != ["null"])
            .unwrap_or(&branches[0]);
        return generate(root, branch, shape);
    }
    let types = types_of(schema);
    let chosen = types
        .iter()
        .copied()
        .find(|kind| *kind != "null")
        .or_else(|| types.first().copied())
        .or_else(|| schema.get("properties").map(|_| "object"))
        .unwrap_or("object");
    match chosen {
        "null" => Value::Null,
        "boolean" => Value::Bool(true),
        "integer" => json!(integer_within(schema)),
        "number" => json!(schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.5)),
        "string" => Value::String(string_for(schema)),
        "array" => {
            let count = schema
                .get("minItems")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .max(1)
                .min(schema.get("maxItems").and_then(Value::as_u64).unwrap_or(1));
            let items = schema.get("items").cloned().unwrap_or(json!({}));
            Value::Array((0..count).map(|_| generate(root, &items, shape)).collect())
        }
        "object" => {
            let mut object = Map::new();
            if schema.get("maxProperties").and_then(Value::as_u64) == Some(0) {
                return Value::Object(object);
            }
            let required: BTreeSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for (key, child) in schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if shape == Shape::Maximal || required.contains(key.as_str()) {
                    object.insert(key.clone(), generate(root, child, shape));
                }
            }
            Value::Object(object)
        }
        other => panic!("unsupported type {other}"),
    }
}

fn integer_within(schema: &Value) -> i64 {
    let minimum = schema.get("minimum").and_then(Value::as_i64);
    let maximum = schema.get("maximum").and_then(Value::as_i64);
    match (minimum, maximum) {
        (Some(low), _) => low.max(1).min(maximum.unwrap_or(i64::MAX)),
        (None, Some(high)) => high.min(1),
        (None, None) => 1,
    }
}

fn string_for(schema: &Value) -> String {
    match schema.get("format").and_then(Value::as_str) {
        Some("uuid") => return "f47ac10b-58cc-4372-a567-0e02b2c3d479".into(),
        Some("date-time") => return "2026-09-25T12:00:00Z".into(),
        Some("date") => return "2026-09-25".into(),
        _ => {}
    }
    if schema.get("pattern").and_then(Value::as_str) == Some(r"^\d{4}-\d{2}-\d{2}$") {
        return "2026-09-25".into();
    }
    let minimum = schema.get("minLength").and_then(Value::as_u64).unwrap_or(1) as usize;
    let maximum = schema
        .get("maxLength")
        .and_then(Value::as_u64)
        .map_or(usize::MAX, |value| value as usize);
    "a".repeat(minimum.max(1).min(maximum))
}

/// Validates `value` against `schema` for exactly the keywords in
/// `UNDERSTOOD_KEYWORDS`.
fn validate(root: &Value, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
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
