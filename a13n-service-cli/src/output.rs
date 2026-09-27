use crate::{CliError, config::Effective};
use a13n::{
    BinaryResponse, Response,
    resources::{PageSource, Pages},
};
use clap::ArgMatches;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::IsTerminal;
use tokio::io::{AsyncWriteExt, stdout};

fn request_id(headers: &reqwest::header::HeaderMap) -> Option<&str> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
}
fn envelope<T: Serialize>(response: Response<T>, config: &Effective) -> Result<Value, CliError> {
    let etag = response.etag().map(str::to_owned);
    let data = serde_json::to_value(response.data)
        .map_err(|_| CliError::protocol("cannot serialize response"))?;
    Ok(if config.include_meta {
        json!({"data":data,"status":response.status.as_u16(), "etag":etag,
            "request_id":request_id(&response.headers),
            "location":response.headers.get("location").and_then(|value| value.to_str().ok())})
    } else {
        data
    })
}
fn table(value: &Value) -> Option<String> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())?;
    let columns = ["id", "name", "status", "kind"];
    let selected: Vec<_> = columns
        .into_iter()
        .filter(|column| items.iter().any(|item| item.get(column).is_some()))
        .collect();
    if selected.is_empty() {
        return None;
    }
    let mut output = format!("{}\n", selected.join("\t"));
    for item in items {
        for (index, column) in selected.iter().enumerate() {
            if index != 0 {
                output.push('\t');
            }
            let field = item.get(column).unwrap_or(&Value::Null);
            let value = field
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| field.to_string());
            output.push_str(&value.replace(['\n', '\t', '\r'], " "));
        }
        output.push('\n');
    }
    Some(output)
}
pub async fn print(value: &Value, config: &Effective) -> Result<(), CliError> {
    let text = if config.format == "table"
        && !config.include_meta
        && let Some(rows) = table(value)
    {
        rows
    } else {
        format!("{value}\n")
    };
    let mut out = stdout();
    out.write_all(text.as_bytes())
        .await
        .map_err(|_| CliError::input("cannot write stdout"))?;
    out.flush()
        .await
        .map_err(|_| CliError::input("cannot flush stdout"))
}
pub async fn response<T: Serialize>(
    response: Response<T>,
    config: &Effective,
) -> Result<(), CliError> {
    print(&envelope(response, config)?, config).await
}
pub async fn pages<S>(mut pages: Pages<S>, config: &Effective) -> Result<(), CliError>
where
    S: PageSource,
    S::Page: Serialize,
{
    let mut output = Vec::new();
    while let Some(page) = pages.next().await? {
        output.push(envelope(page, config)?);
    }
    print(&json!({"pages":output}), config).await
}
pub async fn binary(mut response: BinaryResponse, matches: &ArgMatches) -> Result<(), CliError> {
    let destination = matches.get_one::<String>("output");
    let Some(destination) = destination else {
        return Err(CliError::input(
            "binary response requires --output PATH or --output -",
        ));
    };
    if destination == "-" {
        if std::io::stdout().is_terminal() {
            return Err(CliError::input("refusing to write binary to a terminal"));
        }
        let mut out = stdout();
        while let Some(chunk) = response.chunk().await? {
            out.write_all(&chunk)
                .await
                .map_err(|_| CliError::input("cannot write stdout"))?;
        }
        out.flush()
            .await
            .map_err(|_| CliError::input("cannot flush stdout"))?;
    } else {
        let mut out = tokio::fs::File::create(destination)
            .await
            .map_err(|_| CliError::input("cannot open output file"))?;
        while let Some(chunk) = response.chunk().await? {
            out.write_all(&chunk)
                .await
                .map_err(|_| CliError::input("cannot write output file"))?;
        }
        out.flush()
            .await
            .map_err(|_| CliError::input("cannot flush output file"))?;
    }
    Ok(())
}
