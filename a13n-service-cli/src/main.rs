#![forbid(unsafe_code)]

use a13n::{Client, Error, Response, Secret, generated::models, resources::*};
use clap::{Parser, Subcommand, ValueEnum};
use std::{collections::HashMap, env};

#[derive(Debug, Parser)]
#[command(
    name = "a13n-service-cli",
    version,
    about = "Command-line client for a13n Service"
)]
struct Cli {
    /// Service API base URL.
    #[arg(long, default_value = "http://localhost")]
    base_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read or atomically replace resource labels.
    Labels {
        #[arg(value_enum)]
        resource: Resource,
        id: String,
        /// Explicit workspace ID or key. Credentials never select a workspace.
        #[arg(long)]
        workspace: String,
        /// Complete JSON string-to-string map. Omit to read labels.
        #[arg(long, requires = "if_match")]
        set: Option<String>,
        /// Exact resource ETag required with --set.
        #[arg(long, requires = "set")]
        if_match: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Resource {
    Agent,
    Session,
    Thread,
    Run,
    Skill,
    EnvironmentTemplate,
}

async fn run(cli: Cli) -> Result<(), String> {
    let Command::Labels {
        resource,
        id,
        workspace,
        set,
        if_match,
    } = cli.command;
    let replacement: Option<HashMap<String, String>> = set
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| format!("--set must be a JSON string-to-string map: {error}"))
        })
        .transpose()?;
    let token = env::var("A13N_TOKEN").map_err(|_| "A13N_TOKEN must be set".to_owned())?;
    let client =
        Client::new(&cli.base_url, Secret::new(token)).map_err(|error| error.to_string())?;
    let workspace = client.resources().workspaces().at(workspace);
    let result = match replacement {
        Some(labels) => replace_labels(&workspace, resource, &id, if_match.unwrap(), labels).await,
        None => read_labels(&workspace, resource, &id).await,
    };
    client.close();
    result.map_err(|error| error.to_string())
}

fn print_labels(labels: HashMap<String, String>, etag: Option<&str>) {
    // Keep stdout machine-readable and deterministic; metadata belongs on stderr.
    println!("{}", serde_json::json!({"labels": labels}));
    if let Some(etag) = etag {
        eprintln!("ETag: {etag}");
    }
}

async fn read_labels(
    workspace: &WorkspaceResource<'_>,
    resource: Resource,
    id: &str,
) -> Result<(), Error> {
    macro_rules! read {
        ($collection:ident) => {{
            let response = workspace.$collection().at(id).get().await?;
            print_labels(response.data.labels.clone(), response.etag());
        }};
    }
    match resource {
        Resource::Agent => read!(agents),
        Resource::Session => read!(sessions),
        Resource::Thread => read!(threads),
        Resource::Run => read!(runs),
        Resource::Skill => read!(skills),
        Resource::EnvironmentTemplate => read!(environment_templates),
    }
    Ok(())
}

async fn replace_labels(
    workspace: &WorkspaceResource<'_>,
    resource: Resource,
    id: &str,
    if_match: String,
    labels: HashMap<String, String>,
) -> Result<(), Error> {
    macro_rules! update {
        ($collection:ident, $body:expr, $options:ident) => {{
            let response: Response<_> = workspace
                .$collection()
                .at(id)
                .update(&$body, $options { if_match })
                .await?;
            print_labels(response.data.labels.clone(), response.etag());
        }};
    }
    match resource {
        Resource::Agent => update!(
            agents,
            models::AgentUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            AgentUpdateOptions
        ),
        Resource::Session => update!(
            sessions,
            models::SessionUpdate::new(labels),
            SessionUpdateOptions
        ),
        Resource::Thread => update!(
            threads,
            models::ThreadUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            ThreadUpdateOptions
        ),
        Resource::Run => update!(runs, models::RunLabels::new(labels), RunUpdateOptions),
        Resource::Skill => update!(
            skills,
            models::SkillUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            SkillUpdateOptions
        ),
        Resource::EnvironmentTemplate => update!(
            environment_templates,
            models::TemplateUpdate {
                labels: Some(Some(labels)),
                ..Default::default()
            },
            EnvironmentTemplateUpdateOptions
        ),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
