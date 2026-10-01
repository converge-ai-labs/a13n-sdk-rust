use crate::CliError;
use a13n::UploadFile;
use clap::ArgMatches;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

pub async fn body(raw: &str) -> Result<Value, CliError> {
    let content = if raw == "@-" {
        let mut text = String::new();
        tokio::io::stdin()
            .read_to_string(&mut text)
            .await
            .map_err(|_| CliError::input("cannot read JSON from stdin"))?;
        text
    } else if let Some(path) = raw.strip_prefix('@') {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|_| CliError::input("cannot read JSON file"))?
    } else {
        raw.to_owned()
    };
    serde_json::from_str(&content)
        .map_err(|_| CliError::input("invalid JSON body (use --schema for accepted shape)"))
}
pub fn option<T: DeserializeOwned>(value: &str) -> Result<T, CliError> {
    // Prefer the wire string first: "123" and "true" are valid string selectors.
    // Numeric and boolean target types take the second, unquoted path.
    serde_json::from_value(json!(value))
        .or_else(|_| serde_json::from_str(value))
        .map_err(|_| CliError::input("invalid query/header value"))
}
pub fn optional<T: DeserializeOwned>(
    matches: &ArgMatches,
    key: &str,
) -> Result<Option<T>, CliError> {
    matches
        .get_one::<String>(key)
        .map(|value| option(value))
        .transpose()
}
pub fn many<T: DeserializeOwned>(
    matches: &ArgMatches,
    key: &str,
) -> Result<Option<Vec<T>>, CliError> {
    matches
        .get_many::<String>(key)
        .map(|values| values.map(|value| option(value)).collect())
        .transpose()
}
pub fn schema(index: usize) -> Value {
    let doc: Value =
        serde_json::from_str(include_str!("schemas.json")).expect("generated JSON schemas");
    let mut request = doc["requests"][index.to_string()].clone();
    if let Some(object) = request.as_object_mut() {
        object.insert("components".into(), json!({"schemas": doc["components"]}));
        request
    } else {
        let media = &doc["media_requests"][index.to_string()];
        if media.is_null() {
            json!({"requestBody": null})
        } else {
            json!({"requestBody":media,"components":{"schemas":doc["components"]}})
        }
    }
}
pub fn example(index: usize) -> Value {
    let (name, _, _) = crate::generated::OPERATIONS[index];
    if matches!(name, "healthz get" | "readyz get") {
        json!({"example": crate::generated::EXAMPLES[index]})
    } else {
        json!({"template": crate::generated::EXAMPLES[index], "example": false,
            "note": "Replace uppercase placeholders and provide your own request body matching --schema"})
    }
}
pub fn validate(index: usize, value: &Value) -> Result<(), CliError> {
    let doc: Value =
        serde_json::from_str(include_str!("schemas.json")).expect("generated JSON schemas");
    check(
        value,
        &doc["requests"][index.to_string()],
        &doc["components"],
    )
}
/// Apply the same schema-derived unknown-field check to authored typed inputs.
pub fn validate_model(name: &str, value: &Value) -> Result<(), CliError> {
    let doc: Value =
        serde_json::from_str(include_str!("schemas.json")).expect("generated JSON schemas");
    check(value, &doc["components"][name], &doc["components"])
}
fn compatible(value: &Value, schema: &Value, definitions: &Value) -> bool {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return compatible(
            value,
            &definitions[reference.rsplit('/').next().unwrap_or("")],
            definitions,
        );
    }
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return branches
            .iter()
            .any(|branch| compatible(value, branch, definitions));
    }
    matches!(
        (schema.get("type").and_then(Value::as_str), value),
        (Some("object"), Value::Object(_))
            | (Some("array"), Value::Array(_))
            | (Some("string"), Value::String(_))
            | (Some("null"), Value::Null)
            | (Some("integer" | "number"), Value::Number(_))
            | (Some("boolean"), Value::Bool(_))
    ) || schema.get("type").is_none()
}
fn check(value: &Value, schema: &Value, definitions: &Value) -> Result<(), CliError> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or("");
        return check(value, &definitions[name], definitions);
    }
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        if let Some(discriminator) = schema.get("discriminator")
            && let Some(tag) = discriminator
                .get("propertyName")
                .and_then(Value::as_str)
                .and_then(|name| value.get(name))
                .and_then(Value::as_str)
            && let Some(selected) = discriminator
                .get("mapping")
                .and_then(|mapping| mapping.get(tag))
        {
            return check(value, &json!({"$ref":selected}), definitions);
        }
        let mut candidate = false;
        for branch in branches {
            if !compatible(value, branch, definitions) {
                continue;
            }
            candidate = true;
            if check(value, branch, definitions).is_ok() {
                return Ok(());
            }
        }
        return if candidate {
            Err(CliError::input("request contains an unknown field"))
        } else {
            Ok(())
        };
    }
    if let (Some(map), Some(properties)) = (
        value.as_object(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        for (key, field) in map {
            if let Some(kind) = properties.get(key) {
                check(field, kind, definitions)?;
            } else if let Some(extra) = schema.get("additionalProperties") {
                if extra == &Value::Bool(false) {
                    return Err(CliError::input("request contains an unknown field"));
                }
                if extra.is_object() {
                    check(field, extra, definitions)?;
                }
            } else {
                return Err(CliError::input("request contains an unknown field"));
            }
        }
    } else if let (Some(items), Some(schema)) = (value.as_array(), schema.get("items")) {
        for item in items {
            check(item, schema, definitions)?;
        }
    }
    Ok(())
}
async fn stream(path: &str) -> Result<reqwest::Body, CliError> {
    if path == "-" {
        return Ok(reqwest::Body::wrap_stream(ReaderStream::new(
            tokio::io::stdin(),
        )));
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| CliError::input("cannot open upload file"))?;
    Ok(reqwest::Body::wrap_stream(ReaderStream::new(file)))
}
pub async fn raw_upload(matches: &ArgMatches) -> Result<reqwest::Body, CliError> {
    stream(crate::required(matches, "file")?).await
}
pub async fn upload(matches: &ArgMatches) -> Result<UploadFile, CliError> {
    let path = crate::required(matches, "file")?;
    let name = matches
        .get_one::<String>("file_name")
        .cloned()
        .unwrap_or_else(|| {
            if path == "-" {
                "stdin".to_owned()
            } else {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }
        });
    Ok(UploadFile {
        name,
        content_type: crate::required(matches, "content_type")?.to_owned(),
        body: stream(path).await?,
    })
}
