use crate::{CliError, config::Effective, required};
use a13n::{Client, Response, generated::models, resources::*};
use clap::{Arg, ArgMatches, Command};
use std::collections::HashMap;

pub fn command() -> Command {
    Command::new("labels")
        .about("Read or atomically replace resource labels")
        .arg(Arg::new("resource").required(true).value_parser([
            "agent",
            "session",
            "thread",
            "run",
            "skill",
            "environment-template",
        ]))
        .arg(Arg::new("id").required(true))
        .arg(Arg::new("set").long("set").requires("if_match"))
        .arg(Arg::new("if_match").long("if-match").requires("set"))
}
pub async fn run(
    matches: &ArgMatches,
    client: &Client,
    config: &Effective,
) -> Result<(), CliError> {
    let (_, matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing labels command"))?;
    let resource = required(matches, "resource")?;
    let id = required(matches, "id")?;
    let workspace = client
        .resources()
        .workspaces()
        .at(config.required("workspace_id")?);
    let set: Option<HashMap<String, String>> = matches
        .get_one::<String>("set")
        .map(|value| {
            serde_json::from_str(value)
                .map_err(|_| CliError::input("--set must be a JSON string-to-string map"))
        })
        .transpose()?;
    macro_rules! read {($name:ident) => {{
        let result = workspace.$name().at(id).get().await?;
        let etag = result.etag().map(str::to_owned);
        let value = serde_json::json!({"labels":result.data.labels});
        if config.include_meta {println!("{}",serde_json::json!({"data":value,"status":result.status.as_u16(),"etag":etag,"request_id":result.headers.get("x-request-id").and_then(|value|value.to_str().ok())}));}
        else {println!("{value}");if let Some(etag)=etag{eprintln!("ETag: {etag}");}}
        Ok(())
    }};}
    macro_rules! update {($name:ident, $body:expr, $options:ident) => {{
        let result: Response<_> = workspace.$name().at(id).update(&$body, $options {if_match:required(matches,"if_match")?.to_owned()}).await?;
        let etag = result.etag().map(str::to_owned);
        let value = serde_json::json!({"labels":result.data.labels});
        if config.include_meta {println!("{}",serde_json::json!({"data":value,"status":result.status.as_u16(),"etag":etag,"request_id":result.headers.get("x-request-id").and_then(|value|value.to_str().ok())}));}
        else {println!("{value}");if let Some(etag)=etag{eprintln!("ETag: {etag}");}}
        Ok(())
    }};}
    match (resource, set) {
        ("agent", Some(labels)) => update!(
            agents,
            models::AgentUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            AgentUpdateOptions
        ),
        ("session", Some(labels)) => update!(
            sessions,
            models::SessionUpdate::new(labels),
            SessionUpdateOptions
        ),
        ("thread", Some(labels)) => update!(
            threads,
            models::ThreadUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            ThreadUpdateOptions
        ),
        ("run", Some(labels)) => update!(runs, models::RunLabels::new(labels), RunUpdateOptions),
        ("skill", Some(labels)) => update!(
            skills,
            models::SkillUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            SkillUpdateOptions
        ),
        ("environment-template", Some(labels)) => update!(
            environment_templates,
            models::TemplateUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            EnvironmentTemplateUpdateOptions
        ),
        ("agent", None) => read!(agents),
        ("session", None) => read!(sessions),
        ("thread", None) => read!(threads),
        ("run", None) => read!(runs),
        ("skill", None) => read!(skills),
        ("environment-template", None) => read!(environment_templates),
        _ => Err(CliError::input("invalid labels resource")),
    }
}
