#![forbid(unsafe_code)]
mod config;
mod generated;
mod helpers;
mod input;
mod labels;
mod output;

use a13n::{Client, Error, Secret};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use serde_json::json;
use std::{env, ffi::OsString, time::Duration};

#[derive(Debug)]
pub struct CliError {
    kind: &'static str,
    message: String,
    exit: i32,
    status: Option<u16>,
    code: Option<String>,
    request_id: Option<String>,
}
impl CliError {
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            kind: "input",
            message: message.into(),
            exit: 2,
            status: None,
            code: None,
            request_id: None,
        }
    }
    pub fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: "protocol",
            message: message.into(),
            exit: 5,
            status: None,
            code: None,
            request_id: None,
        }
    }
}
impl From<Error> for CliError {
    fn from(error: Error) -> Self {
        match error {
            Error::Api(api) => {
                let (kind, exit) = match api.status { 401 => ("auth", 3), 403 => ("permission", 3), 409 | 412 | 428 => ("conflict", 4), 400..=499 => ("request", 2), _ => ("service", 5) };
                Self {kind,message:format!("Service returned HTTP {} ({kind}); inspect the request and --schema", api.status),exit,
                    status:Some(api.status),code:Some(api.code),request_id:api.request_id}
            }
            Error::Transport(_) => Self {kind:"transport",message:"Service transport failed. A mutation may have succeeded; inspect server state before replaying it.".into(),exit:5,status:None,code:None,request_id:None},
            Error::Protocol(_) => Self::protocol("Service response violates the pinned contract"),
            Error::Submission(error) => Self::protocol(format!("Entry {} settled as {} without incorporation", error.entry_id, error.entry.data.status)),
            Error::Timeout => Self::protocol("Local observation deadline elapsed; remote work may continue"),
            Error::InvalidInput => Self::input("invalid URL, resource identifier or request precondition"),
            Error::Closed => Self::protocol("client closed during operation"),
        }
    }
}
fn required<'a>(matches: &'a ArgMatches, name: &str) -> Result<&'a str, CliError> {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| CliError::input(format!("missing --{name}")))
}
fn command() -> Command {
    let api = helpers::commands(generated::commands());
    Command::new("a13n-service-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Command-line client for a13n Service (complete generated API)")
        .arg_required_else_help(true)
        .subcommand_required(true)
        .arg(
            Arg::new("base_url")
                .long("base-url")
                .global(true)
                .help("Service origin"),
        )
        .arg(Arg::new("workspace").long("workspace").global(true).help(
            "Session workspace ID; API keys already select their workspace (omit for API keys)",
        ))
        .arg(
            Arg::new("organization")
                .long("organization")
                .global(true)
                .help("Explicit organization ID or key"),
        )
        .arg(
            Arg::new("profile")
                .long("profile")
                .global(true)
                .help("Named configuration profile"),
        )
        .arg(
            Arg::new("ca_bundle")
                .long("ca-bundle")
                .global(true)
                .help("Trusted PEM CA bundle for HTTPS"),
        )
        .arg(
            Arg::new("include_meta")
                .long("include-meta")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Include status, ETag, request ID and redirect Location"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .global(true)
                .default_value("json")
                .value_parser(["json", "table"]),
        )
        .arg(
            Arg::new("error_format")
                .long("error-format")
                .global(true)
                .default_value("human")
                .value_parser(["human", "json"]),
        )
        .arg(
            Arg::new("dry_run")
                .long("dry-run")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Local request plan, without contacting Service"),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .global(true)
                .default_value("300")
                .value_parser(value_parser!(u64).range(1..))
                .help("Whole-operation timeout in seconds"),
        )
        .subcommand(
            Command::new("config")
                .about("Inspect effective non-secret settings")
                .subcommand_required(true)
                .subcommand(Command::new("show")),
        )
        .subcommand(
            Command::new("completion")
                .about("Generate offline shell completion")
                .arg(Arg::new("shell").required(true).value_parser([
                    "bash",
                    "zsh",
                    "fish",
                    "powershell",
                    "elvish",
                ])),
        )
        .subcommands(api.get_subcommands().cloned())
        .subcommand(labels::command())
}
fn plan(
    index: usize,
    matches: &ArgMatches,
    config: &config::Effective,
) -> Result<serde_json::Value, CliError> {
    let (_, method, template) = generated::OPERATIONS[index];
    let mut path = template.to_owned();
    for capture in template.split('{').skip(1) {
        let Some(name) = capture.split('}').next() else {
            continue;
        };
        let argument = |key| {
            matches
                .try_get_one::<String>(key)
                .ok()
                .flatten()
                .map(String::as_str)
        };
        let value = match name {
            "workspace_id" => {
                if template.ends_with("{workspace_id}") {
                    argument("id")
                } else {
                    config.workspace.as_deref()
                }
            }
            "organization_id" => {
                if template.ends_with("{organization_id}") {
                    argument("id")
                } else {
                    config.organization.as_deref()
                }
            }
            other => argument(other).or_else(|| argument("id")),
        };
        let value = value.ok_or_else(|| {
            CliError::input(format!("missing target {name}; pass its scope or ID"))
        })?;
        if value.is_empty() || matches!(value, "." | "..") {
            return Err(CliError::input("invalid resource identifier"));
        }
        let mut url = reqwest::Url::parse("https://example.invalid/").expect("static URL");
        url.path_segments_mut()
            .expect("static URL path")
            .push(value);
        path = path.replace(&format!("{{{name}}}"), url.path().trim_start_matches('/'));
    }
    Ok(
        json!({"method":method,"path":path,"body":if matches.try_get_one::<String>("body").ok().flatten().is_some() || matches.try_get_one::<String>("file").ok().flatten().is_some() {"[REDACTED]"} else {"none"},
        "preconditions":{"if_match":matches.try_get_one::<String>("if_match").ok().flatten().is_some(),"idempotency_key":matches.try_get_one::<String>("idempotency_key").ok().flatten().is_some()},
        "local_only":true,"server_validated":false}),
    )
}
async fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error
                .print()
                .map_err(|_| CliError::input("cannot print help"))?;
            return Ok(());
        }
        Err(error) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument => {
            return Err(CliError::input(error.to_string()));
        }
        Err(error) if error.kind() == clap::error::ErrorKind::InvalidValue => {
            return Err(CliError::input(
                "invalid value; use --help for allowed values",
            ));
        }
        Err(_) => {
            return Err(CliError::input(
                "invalid command arguments; use --help for required flags and positional IDs",
            ));
        }
    };
    if let Some(("completion", command)) = matches.subcommand() {
        let shell = match required(command, "shell")? {
            "bash" => clap_complete::Shell::Bash,
            "zsh" => clap_complete::Shell::Zsh,
            "fish" => clap_complete::Shell::Fish,
            "powershell" => clap_complete::Shell::PowerShell,
            _ => clap_complete::Shell::Elvish,
        };
        clap_complete::generate(
            shell,
            &mut self::command(),
            "a13n-service-cli",
            &mut std::io::stdout(),
        );
        return Ok(());
    }
    if let Some((index, leaf)) = generated::selected(&matches) {
        if leaf.get_flag("schema") {
            println!("{}", input::schema(index));
            return Ok(());
        }
        if leaf.get_flag("example") {
            println!("{}", input::example(index));
            return Ok(());
        }
    }
    let config = config::load(&matches)?;
    if matches.subcommand_name() == Some("config") {
        println!("{}", config.show());
        return Ok(());
    }
    if matches.get_flag("dry_run") {
        if let Some((index, leaf)) = generated::selected(&matches) {
            println!("{}", plan(index, leaf, &config)?);
            return Ok(());
        }
        return Err(CliError::input(
            "--dry-run is available only for generated API commands; semantic and observation commands have no local plan",
        ));
    }
    if let Some((index, leaf)) = generated::selected(&matches)
        && generated::OPERATIONS[index].1 == "GET"
        && leaf.try_get_one::<String>("output").is_ok()
        && leaf.get_one::<String>("output").is_none()
    {
        return Err(CliError::input(
            "binary content requires --output PATH or --output -",
        ));
    }
    let mut builder = Client::builder(&config.base_url);
    if let Some(workspace) = &config.workspace {
        builder = builder.workspace(workspace.clone());
    }
    if let Ok(token) = env::var(&config.token_env)
        && !token.is_empty()
    {
        builder = builder.bearer(Secret::new(token));
    }
    if let Some(bundle) = &config.ca_bundle {
        let pem = std::fs::read(bundle).map_err(|_| CliError::input("cannot read CA bundle"))?;
        let certificate = reqwest::Certificate::from_pem(&pem)
            .map_err(|_| CliError::input("invalid PEM CA bundle"))?;
        builder = builder.http_builder(
            reqwest::Client::builder()
                .no_proxy()
                .add_root_certificate(certificate),
        );
    }
    let client = builder.build()?;
    let operation = async {
        if let Some((index, leaf)) = generated::selected(&matches) {
            generated::dispatch(index, leaf, &client, &config).await
        } else if matches.subcommand_name() == Some("labels") {
            labels::run(&matches, &client, &config).await
        } else {
            helpers::run(&matches, &client, &config).await
        }
    };
    let result = tokio::select! {
        _ = tokio::signal::ctrl_c() => Err(CliError {kind:"cancelled",message:"Local operation interrupted; remote work may continue".into(),exit:130,status:None,code:None,request_id:None}),
        value = tokio::time::timeout(Duration::from_secs(config.timeout), operation) => value.unwrap_or_else(|_| Err(CliError {kind:"timeout",message:"Local timeout; remote mutation may have succeeded".into(),exit:5,status:None,code:None,request_id:None})),
    };
    client.close();
    result
}
#[tokio::main]
async fn main() {
    let args: Vec<_> = env::args_os().collect();
    let json_errors = args.iter().any(|part| part == "--error-format=json")
        || args
            .windows(2)
            .any(|pair| pair[0] == "--error-format" && pair[1] == "json");
    if let Err(error) = run(args).await {
        if json_errors {
            eprintln!(
                "{}",
                json!({"error":{"kind":error.kind,"message":error.message,"status":error.status,"code":error.code,"request_id":error.request_id}})
            );
        } else {
            eprintln!("error: {}", error.message);
        }
        std::process::exit(error.exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_commands_have_offline_schema_for_every_operation() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../contract/openapi.json")).unwrap();
        let count: usize = contract["paths"]
            .as_object()
            .unwrap()
            .values()
            .map(|path| {
                path.as_object()
                    .unwrap()
                    .keys()
                    .filter(|method| {
                        matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete")
                    })
                    .count()
            })
            .sum();
        assert_eq!(generated::OPERATIONS.len(), count);
        for (name, _, _) in generated::OPERATIONS {
            let arguments = std::iter::once("a13n-service-cli".to_owned())
                .chain(name.split_whitespace().map(str::to_owned))
                .chain(std::iter::once("--schema".to_owned()));
            let matches = command()
                .try_get_matches_from(arguments)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let (index, leaf) = generated::selected(&matches).expect("dispatch mapping");
            assert!(leaf.get_flag("schema"));
            let schema = input::schema(index);
            assert!(schema.get("components").is_some() || schema.get("requestBody").is_some());
        }
    }
    #[test]
    fn string_selectors_and_discriminated_payload_typos() {
        let text: String = input::option("123").unwrap();
        assert_eq!(text, "123");
        let text: String = input::option("true").unwrap();
        assert_eq!(text, "true");
        let number: i32 = input::option("123").unwrap();
        assert_eq!(number, 123);
        let index = generated::OPERATIONS
            .iter()
            .position(|item| item.0 == "threads create")
            .unwrap();
        let payload = json!({"agent_id":"agt_example","payload":{"content":[{"type":"text","text":"hello","txet":"typo"}]}});
        assert!(input::validate(index, &payload).is_err());
    }
    #[test]
    fn rejects_nested_credentials_typo_without_rejecting_open_config() {
        let index = generated::OPERATIONS
            .iter()
            .position(|item| item.0 == "connections create")
            .unwrap();
        let bad = json!({"type":"mcp","name":"test","config":{"url":"https://example.net"},
            "credential":{"token":"fixture-value","tokne":"bad"}});
        assert!(input::validate(index, &bad).is_err());
        let good = json!({"type":"mcp","name":"test","config":{"url":"https://example.net"},
            "credential":{"token":"fixture-value"}});
        assert!(input::validate(index, &good).is_ok());
    }
    #[test]
    fn event_item_labels_use_wire_snake_case() {
        assert_eq!(
            helpers::item_kind(&a13n::streaming::ItemKind::TextMessage),
            "text_message"
        );
        assert_eq!(
            helpers::item_kind(&a13n::streaming::ItemKind::ToolCall),
            "tool_call"
        );
        assert_eq!(
            helpers::item_state(&a13n::streaming::ItemState::InProgress),
            "in_progress"
        );
    }
    #[test]
    fn offline_plan_has_no_undefined_arg_panics() {
        for args in [
            vec!["a13n-service-cli", "healthz", "get", "--dry-run"],
            vec![
                "a13n-service-cli",
                "agents",
                "get",
                "agt_example",
                "--workspace",
                "ws",
                "--dry-run",
            ],
        ] {
            let matches = command().try_get_matches_from(args).unwrap();
            let (index, leaf) = generated::selected(&matches).unwrap();
            let config = config::load(&matches).unwrap();
            assert!(
                plan(index, leaf, &config).unwrap()["local_only"]
                    .as_bool()
                    .unwrap()
            );
        }
    }
}
