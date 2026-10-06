use crate::{CliError, config::Effective, input, output, required};
use a13n::{
    Client, SendOptions, StartOptions,
    generated::models,
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub fn commands(mut api: Command) -> Command {
    api = api.mut_subcommand("agents", |agents| {
        agents
        .subcommand(Command::new("start")
            .about("Submit the first message to an Agent in a new Thread")
            .arg(Arg::new("id").required(true).help("Agent ID"))
            .arg(Arg::new("text").long("text").required_unless_present("payload").conflicts_with("payload"))
            .arg(Arg::new("payload").long("payload").help("Typed MessagePayload JSON, @file or @- (instead of --text)"))
            .arg(Arg::new("options").long("options").help("Complete RunOptions JSON, @file or @-; configuration is a snapshot, not a merge"))
            .arg(Arg::new("idempotency_key").long("idempotency-key").required(true))
            .arg(Arg::new("wait").long("wait").action(ArgAction::SetTrue)))
        .subcommand(Command::new("send")
            .about("Continue a Thread using the specified Agent; Threads have no permanent Agent")
            .arg(Arg::new("id").required(true).help("Agent ID"))
            .arg(Arg::new("thread_id").long("thread").required(true))
            .arg(Arg::new("text").long("text").required_unless_present("payload").conflicts_with("payload"))
            .arg(Arg::new("payload").long("payload").help("Typed MessagePayload JSON, @file or @- (instead of --text)"))
            .arg(Arg::new("options").long("options").help("Complete RunOptions JSON, @file or @-; configuration is a snapshot, not a merge"))
            .arg(Arg::new("idempotency_key").long("idempotency-key").required(true))
            .arg(Arg::new("wait").long("wait").action(ArgAction::SetTrue)))
    });
    api = api.mut_subcommand("runs", |runs| runs.subcommand(Command::new("wait")
        .about("Wait for exactly this Run to reach completed, waiting, failed or cancelled; never follows successors")
        .arg(Arg::new("id").required(true))
        .arg(Arg::new("interval_ms").long("interval-ms").default_value("500").value_parser(value_parser!(u64).range(1..))))
        .subcommand(Command::new("result")
            .about("Read authoritative committed items of one Run")
            .arg(Arg::new("id").required(true))));
    api.mut_subcommand("threads", |threads| {
        threads.subcommand(Command::new("events")
            .about("Observe Thread frames as JSONL; changed/gap/reset are readback hints, not reconstructed state")
            .arg(Arg::new("thread_id").long("thread").required(true))
            .arg(Arg::new("after").long("after").help("Last applied Redis cursor; a seek hint when run/position are supplied"))
            .arg(Arg::new("run").long("run").requires("position").help("Run owning applied display coverage (requires --position)"))
            .arg(Arg::new("position").long("position").requires("run").help("Applied attempt-sequence display position (requires --run)"))
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
    let resources = client.resources();
    match matches.subcommand() {
        Some(("agents", agents)) => {
            let (action, leaf) = agents
                .subcommand()
                .ok_or_else(|| CliError::input("missing Agent command"))?;
            let agent = client.agent(required(leaf, "id")?);
            let key = required(leaf, "idempotency_key")?;
            let payload = if let Some(raw) = leaf.get_one::<String>("payload") {
                let value = input::body(raw).await?;
                input::validate_model("MessagePayload", &value)?;
                serde_json::from_value::<models::MessagePayload>(value)
                    .map_err(|_| CliError::input("payload does not match MessagePayload schema"))?
            } else {
                a13n::text_payload(required(leaf, "text")?)
            };
            let options = if let Some(raw) = leaf.get_one::<String>("options") {
                let value = input::body(raw).await?;
                input::validate_model("RunOptions-Input", &value)?;
                Some(Box::new(
                    serde_json::from_value::<models::RunOptionsInput>(value)
                        .map_err(|_| CliError::input("options do not match RunOptions schema"))?,
                ))
            } else {
                None
            };
            let mut interaction = match action {
                "start" => {
                    agent
                        .start_with(
                            payload,
                            key,
                            StartOptions {
                                options,
                                ..Default::default()
                            },
                        )
                        .await?
                }
                "send" => {
                    agent
                        .send_with(
                            required(leaf, "thread_id")?,
                            payload,
                            key,
                            SendOptions {
                                options,
                                ..Default::default()
                            },
                        )
                        .await?
                }
                _ => return Err(CliError::input("unknown Agent command")),
            };
            if leaf.get_flag("wait") {
                let outcome = interaction.result().await?;
                let receipt = output::envelope(interaction.receipt, config)?;
                let run = output::envelope(outcome.snapshot, config)?;
                output::print(&json!({"submitted":receipt,"outcome":run}), config).await
            } else {
                output::response(interaction.receipt, config).await
            }
        }
        Some(("runs", runs)) => match runs.subcommand() {
            Some(("wait", leaf)) => {
                let interval =
                    Duration::from_millis(*leaf.get_one::<u64>("interval_ms").unwrap_or(&500));
                let outcome = client
                    .run(required(leaf, "id")?)
                    .wait_with(Duration::from_secs(config.timeout), interval)
                    .await?;
                output::response(outcome.snapshot, config).await
            }
            Some(("result", leaf)) => {
                let result = resources
                    .runs()
                    .at(required(leaf, "id")?)
                    .items()
                    .get(Default::default())
                    .await?;
                output::response(result, config).await
            }
            _ => Err(CliError::input("unknown Run command")),
        },
        Some(("threads", threads)) => match threads.subcommand() {
            Some(("inbox", inbox)) => {
                let (_, leaf) = inbox
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing wait command"))?;
                let interval =
                    Duration::from_millis(*leaf.get_one::<u64>("interval_ms").unwrap_or(&500));
                let result = client
                    .entry(required(leaf, "thread_id")?, required(leaf, "id")?)
                    .wait(interval)
                    .await?;
                output::response(result, config).await
            }
            Some(("events", leaf)) => {
                let thread = resources.threads().at(required(leaf, "thread_id")?);
                let mut stream = ThreadStream::open(
                    thread,
                    StreamOptions {
                        after: leaf.get_one::<String>("after").cloned(),
                        run: leaf.get_one::<String>("run").cloned(),
                        position: leaf.get_one::<String>("position").cloned(),
                        max_reconnects: *leaf.get_one::<usize>("max_reconnects").unwrap_or(&0),
                        ..Default::default()
                    },
                )
                .await?;
                let mut stdout = tokio::io::stdout();
                while let Some(frame) = stream.next().await? {
                    let applied = stream.applied_cursor();
                    let applied_position = stream.applied_position();
                    let event = match frame {
                        ThreadFrame::Delta { cursor, data } => {
                            json!({"type":"delta","cursor":cursor,"applied_cursor":applied,"applied_position":applied_position,"run_id":data.run_id,"attempt":data.attempt,"sequence":data.sequence,"event":data.event,"item":data.item})
                        }
                        ThreadFrame::Boundary { cursor, data } => {
                            json!({"type":"boundary","cursor":cursor,"applied_cursor":applied,"applied_position":applied_position,"run_id":data.run_id,"attempt":data.attempt,"sequence":data.sequence})
                        }
                        ThreadFrame::Changed(data) => {
                            json!({"type":"changed","version":data.version,"readback":"thread"})
                        }
                        ThreadFrame::Gap(data) => {
                            json!({"type":"gap","run_id":data.run_id,"position":data.position,"readback":"run_items"})
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
