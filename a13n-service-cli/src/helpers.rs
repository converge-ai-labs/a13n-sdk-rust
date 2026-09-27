use crate::{CliError, config::Effective, output, required};
use a13n::{
    Client,
    streaming::{ItemKind, ItemState, StreamOptions, ThreadFrame},
};
pub(crate) fn item_kind(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::TextMessage => "text_message",
        ItemKind::ReasoningMessage => "reasoning_message",
        ItemKind::ToolCall => "tool_call",
        ItemKind::Observation => "observation",
    }
}
pub(crate) fn item_state(state: &ItemState) -> &'static str {
    match state {
        ItemState::InProgress => "in_progress",
        ItemState::Completed => "completed",
        ItemState::Interrupted => "interrupted",
        ItemState::Failed => "failed",
    }
}
use clap::{Arg, ArgMatches, Command, value_parser};
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub fn commands(mut api: Command) -> Command {
    api = api.mut_subcommand("runs", |runs| runs.subcommand(Command::new("wait")
        .about("Wait for exactly this Run to reach completed, waiting, failed or cancelled; never follows successors")
        .arg(Arg::new("id").required(true))
        .arg(Arg::new("interval_ms").long("interval-ms").default_value("500").value_parser(value_parser!(u64).range(1..)))));
    api.mut_subcommand("threads", |threads| {
        threads.subcommand(Command::new("events")
            .about("Observe Thread frames as JSONL; changed/gap/reset are readback hints, not reconstructed state")
            .arg(Arg::new("thread_id").long("thread").required(true))
            .arg(Arg::new("after").long("after").help("Last applied cursor, not last received cursor"))
            .arg(Arg::new("max_reconnects").long("max-reconnects").default_value("0").value_parser(value_parser!(usize))))
            .mut_subcommand("inbox", |inbox| inbox.subcommand(Command::new("wait")
                .about("Wait for exactly this Entry until consumed, failed or withdrawn; assignment is not consumption")
                .arg(Arg::new("id").required(true)).arg(Arg::new("thread_id").long("thread").required(true))
                .arg(Arg::new("interval_ms").long("interval-ms").default_value("500").value_parser(value_parser!(u64).range(1..)))))
    })
}
pub async fn run(
    matches: &ArgMatches,
    client: &Client,
    config: &Effective,
) -> Result<(), CliError> {
    let workspace = client
        .resources()
        .workspaces()
        .at(config.required("workspace_id")?);
    match matches.subcommand() {
        Some(("runs", runs)) => {
            let (_, leaf) = runs
                .subcommand()
                .ok_or_else(|| CliError::input("missing wait command"))?;
            let interval =
                Duration::from_millis(*leaf.get_one::<u64>("interval_ms").unwrap_or(&500));
            let result = workspace
                .runs()
                .at(required(leaf, "id")?)
                .wait(interval)
                .await?;
            output::response(result, config).await
        }
        Some(("threads", threads)) => match threads.subcommand() {
            Some(("inbox", inbox)) => {
                let (_, leaf) = inbox
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing wait command"))?;
                let interval =
                    Duration::from_millis(*leaf.get_one::<u64>("interval_ms").unwrap_or(&500));
                let result = workspace
                    .threads()
                    .at(required(leaf, "thread_id")?)
                    .inbox()
                    .at(required(leaf, "id")?)
                    .wait(interval)
                    .await?;
                output::response(result, config).await
            }
            Some(("events", leaf)) => {
                let thread = workspace.threads().at(required(leaf, "thread_id")?);
                let mut stream = thread
                    .events(StreamOptions {
                        after: leaf.get_one::<String>("after").cloned(),
                        max_reconnects: *leaf.get_one::<usize>("max_reconnects").unwrap_or(&0),
                        ..Default::default()
                    })
                    .await?;
                let mut stdout = tokio::io::stdout();
                while let Some(frame) = stream.next().await? {
                    let applied = stream.applied_cursor();
                    let event = match frame {
                        ThreadFrame::Delta { cursor, data } => {
                            json!({"type":"delta","cursor":cursor,"applied_cursor":applied,"run_id":data.run_id,"attempt":data.attempt,"sequence":data.sequence,"event":data.event,"item":data.item.as_ref().map(|item|json!({"id":item.id,"kind":item_kind(&item.kind),"state":item_state(&item.state)}))})
                        }
                        ThreadFrame::Boundary { cursor, data } => {
                            json!({"type":"boundary","cursor":cursor,"applied_cursor":applied,"run_id":data.run_id,"attempt":data.attempt,"sequence":data.sequence})
                        }
                        ThreadFrame::Changed(data) => {
                            json!({"type":"changed","version":data.version,"readback":"thread"})
                        }
                        ThreadFrame::Gap(data) => {
                            json!({"type":"gap","run_id":data.run_id,"readback":"run_items"})
                        }
                        ThreadFrame::Reset(data) => {
                            json!({"type":"reset","run_id":data.run_id,"readback":"run_items"})
                        }
                    };
                    stdout
                        .write_all(format!("{event}\n").as_bytes())
                        .await
                        .map_err(|_| CliError::input("cannot write event stream"))?;
                    stdout
                        .flush()
                        .await
                        .map_err(|_| CliError::input("cannot flush event stream"))?;
                }
                Ok(())
            }
            _ => Err(CliError::input("unknown Thread helper")),
        },
        _ => Err(CliError::input("unknown helper")),
    }
}
