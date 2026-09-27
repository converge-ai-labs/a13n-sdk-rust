use crate::CliError;
use clap::ArgMatches;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, env, path::PathBuf};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    profiles: HashMap<String, Profile>,
    #[serde(default)]
    defaults: Profile,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    base_url: Option<String>,
    workspace: Option<String>,
    organization: Option<String>,
    ca_bundle: Option<String>,
    token_env: Option<String>,
}

pub struct Effective {
    pub base_url: String,
    pub workspace: Option<String>,
    pub organization: Option<String>,
    pub ca_bundle: Option<String>,
    pub token_env: String,
    pub include_meta: bool,
    pub format: String,
    pub timeout: u64,
    pub sources: Value,
}
impl Effective {
    pub fn required(&self, name: &str) -> Result<String, CliError> {
        match name {
            "workspace_id" => self.workspace.clone(),
            "organization_id" => self.organization.clone(),
            _ => None,
        }
        .ok_or_else(|| {
            CliError::input(format!(
                "{name} is required; pass --{}",
                name.trim_end_matches("_id")
            ))
        })
    }
    pub fn show(&self) -> Value {
        json!({"base_url":self.base_url,"workspace":self.workspace,"organization":self.organization,
            "ca_bundle":self.ca_bundle,"token_env":self.token_env,"sources":self.sources})
    }
}
fn select(
    cli: &ArgMatches,
    name: &str,
    env_name: &str,
    profile: &Option<String>,
    explicit: bool,
) -> (Option<String>, &'static str) {
    if let Some(value) = cli.get_one::<String>(name) {
        return (Some(value.clone()), "cli");
    }
    if explicit {
        return (profile.clone(), "profile");
    }
    if let Ok(value) = env::var(env_name) {
        return (Some(value), "environment");
    }
    (profile.clone(), "default")
}
pub fn load(cli: &ArgMatches) -> Result<Effective, CliError> {
    let explicit_config = env::var_os("A13N_CLI_CONFIG").map(PathBuf::from);
    let config_path = explicit_config.clone().or_else(|| {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/a13n-service-cli/config.json"))
    });
    let config = if let Some(path) =
        config_path.filter(|path| path.exists() || explicit_config.is_some())
    {
        let bytes = std::fs::read(path).map_err(|_| CliError::input("cannot read CLI config"))?;
        serde_json::from_slice::<Config>(&bytes)
            .map_err(|_| CliError::input("invalid CLI config JSON"))?
    } else {
        Config::default()
    };
    let selected = cli.get_one::<String>("profile");
    let explicit = selected.is_some();
    let profile = match selected {
        Some(name) => config
            .profiles
            .get(name)
            .ok_or_else(|| CliError::input("unknown profile"))?,
        None => &config.defaults,
    };
    let (base_url, base_src) = select(
        cli,
        "base_url",
        "A13N_BASE_URL",
        &profile.base_url,
        explicit,
    );
    let (workspace, workspace_src) = select(
        cli,
        "workspace",
        "A13N_WORKSPACE",
        &profile.workspace,
        explicit,
    );
    let (organization, organization_src) = select(
        cli,
        "organization",
        "A13N_ORGANIZATION",
        &profile.organization,
        explicit,
    );
    let (ca_bundle, ca_src) = select(
        cli,
        "ca_bundle",
        "A13N_CA_BUNDLE",
        &profile.ca_bundle,
        explicit,
    );
    let (token_env, token_src) = if explicit {
        (profile.token_env.clone(), "profile")
    } else {
        (profile.token_env.clone(), "default")
    };
    let base_url = base_url.unwrap_or_else(|| "http://localhost".into());
    let token_env = token_env.unwrap_or_else(|| "A13N_TOKEN".into());
    if token_env.is_empty() || token_env.contains('=') {
        return Err(CliError::input("invalid token_env name"));
    }
    Ok(Effective {
        base_url,
        workspace,
        organization,
        ca_bundle,
        token_env,
        include_meta: cli.get_flag("include_meta"),
        format: cli
            .get_one::<String>("format")
            .cloned()
            .unwrap_or_else(|| "json".into()),
        timeout: *cli.get_one::<u64>("timeout").unwrap_or(&30),
        sources: json!({"base_url":base_src,"workspace":workspace_src,"organization":organization_src,
            "ca_bundle":ca_src,"token_env":token_src}),
    })
}
