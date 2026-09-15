use std::{
    io::{self, IsTerminal, Read, Write},
    process::ExitCode,
    str::FromStr,
    sync::Arc,
};

use clap::{Args, Parser, Subcommand, ValueEnum, error::ErrorKind};
use dialoguer::Select;
use serde::Serialize;
use serde_json::{Map, Value};
use yaait::{
    AddRequest, App, AppPaths, CachePolicy, FileRegistry, ProviderDescriptor, ProviderId,
    SetupFieldKind, SetupInput, TrackerError, TrackerId, UsageOptions,
    application::{RemovedData, ServiceResult},
    presentation::{Envelope, human},
    providers::{GitHubCopilotProvider, LiteLlmProvider},
};

#[derive(Parser)]
#[command(name = "yaait", version, about = "Multi-instance AI usage tracker")]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Json)]
    format: OutputFormat,
    #[arg(long, global = true, conflicts_with = "format")]
    human: bool,
    #[arg(long, global = true)]
    pretty: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    Human,
}

#[derive(Subcommand)]
enum Command {
    Providers(ProvidersArgs),
    Add(AddArgs),
    List,
    Show { tracker_id: String },
    Setup(SetupArgs),
    Enable { tracker_id: String },
    Disable { tracker_id: String },
    Remove(RemoveArgs),
    Usage(UsageArgs),
    Debug,
}

#[derive(Serialize)]
struct DebugData {
    app_dir: std::path::PathBuf,
    cache_dir: std::path::PathBuf,
}

#[derive(Args)]
struct ProvidersArgs {
    #[command(subcommand)]
    command: ProvidersCommand,
}

#[derive(Subcommand)]
enum ProvidersCommand {
    List,
    Describe { provider_id: String },
}

#[derive(Args)]
struct AddArgs {
    #[arg(long)]
    provider: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    input: Option<String>,
    tracker_id: String,
}

#[derive(Args)]
struct SetupArgs {
    #[arg(long)]
    input: Option<String>,
    tracker_id: String,
}

#[derive(Args)]
struct RemoveArgs {
    #[arg(long)]
    yes: bool,
    tracker_id: String,
}

#[derive(Args)]
struct UsageArgs {
    #[arg(long = "tracker")]
    trackers: Vec<String>,
    #[arg(long)]
    details: bool,
    /// Bypass cached usage reports and query every selected tracker
    #[arg(long)]
    refresh: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let raw = error.to_string();
            let line = raw.lines().next().unwrap_or("invalid arguments");
            let message = line.strip_prefix("error: ").unwrap_or(line);
            let envelope = Envelope::failure("cli", TrackerError::invalid(message));
            return emit(&envelope, requested_format(), false);
        }
    };
    let format = if cli.human {
        OutputFormat::Human
    } else {
        cli.format
    };
    let pretty = cli.pretty;
    let command_name = canonical_name(&cli.command);
    let result = run(cli).await;
    let envelope = match result {
        Ok(envelope) => envelope,
        Err(error) => Envelope::failure(command_name, error),
    };
    emit(&envelope, format, pretty)
}

async fn run(cli: Cli) -> Result<Envelope, TrackerError> {
    let paths = AppPaths::discover()?;
    if matches!(&cli.command, Command::Debug) {
        return Ok(Envelope::success(
            "debug",
            DebugData {
                app_dir: paths.data_root,
                cache_dir: paths.cache_root,
            },
            Vec::new(),
        ));
    }

    let registry = FileRegistry::new(paths);
    let mut app = App::new(registry)?;
    app.register_provider(Arc::new(GitHubCopilotProvider::default()));
    app.register_provider(Arc::new(LiteLlmProvider::default()));

    match cli.command {
        Command::Providers(args) => match args.command {
            ProvidersCommand::List => Ok(envelope("providers.list", app.providers())),
            ProvidersCommand::Describe { provider_id } => {
                let id = ProviderId::from_str(&provider_id)?;
                Ok(envelope("providers.describe", app.describe_provider(&id)?))
            }
        },
        Command::Add(args) => {
            let id = TrackerId::from_str(&args.tracker_id)?;
            let provider_id = ProviderId::from_str(&args.provider)?;
            let descriptor = app.describe_provider(&provider_id)?.data.provider;
            let input = complete_input(&descriptor, read_input(args.input)?)?;
            Ok(envelope(
                "add",
                app.add(AddRequest {
                    id,
                    provider: provider_id,
                    name: args.name,
                    description: args.description,
                    input,
                })
                .await?,
            ))
        }
        Command::List => Ok(envelope("list", app.list()?)),
        Command::Show { tracker_id } => {
            let id = TrackerId::from_str(&tracker_id)?;
            Ok(envelope("show", app.show(&id)?))
        }
        Command::Setup(args) => {
            let id = TrackerId::from_str(&args.tracker_id)?;
            let detail = app.show(&id)?.data.tracker;
            let descriptor = app
                .describe_provider(&detail.summary.provider)?
                .data
                .provider;
            let input = complete_input(&descriptor, read_input(args.input)?)?;
            Ok(envelope("setup", app.setup(&id, input).await?))
        }
        Command::Enable { tracker_id } => {
            let id = TrackerId::from_str(&tracker_id)?;
            Ok(envelope("enable", app.set_enabled(&id, true)?))
        }
        Command::Disable { tracker_id } => {
            let id = TrackerId::from_str(&tracker_id)?;
            Ok(envelope("disable", app.set_enabled(&id, false)?))
        }
        Command::Remove(args) => {
            let id = TrackerId::from_str(&args.tracker_id)?;
            if !args.yes {
                if !io::stdin().is_terminal() {
                    return Err(TrackerError::invalid(
                        "remove requires --yes when input is not a terminal",
                    ));
                }
                eprint!("Remove tracker {id}? [y/N] ");
                io::stderr().flush().ok();
                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .map_err(|_| TrackerError::invalid("could not read confirmation"))?;
                if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
                    return Ok(Envelope::success(
                        "remove",
                        RemovedData { removed: None },
                        Vec::new(),
                    ));
                }
            }
            Ok(envelope("remove", app.remove(&id)?))
        }
        Command::Usage(args) => {
            let filters = args
                .trackers
                .iter()
                .map(|value| TrackerId::from_str(value))
                .collect::<Result<Vec<_>, _>>()?;
            let result = app
                .usage(
                    &filters,
                    UsageOptions {
                        details: args.details,
                        cache: if args.refresh {
                            CachePolicy::Refresh
                        } else {
                            CachePolicy::Cached
                        },
                    },
                )
                .await?;
            Ok(Envelope::usage(result.data, result.warnings, result.errors))
        }
        Command::Debug => unreachable!("debug returns before application initialization"),
    }
}

fn envelope<T: serde::Serialize>(command: &str, result: ServiceResult<T>) -> Envelope {
    Envelope::success(command, result.data, result.warnings)
}

fn read_input(argument: Option<String>) -> Result<SetupInput, TrackerError> {
    let Some(argument) = argument else {
        return Ok(Map::new());
    };
    if argument != "-" {
        return Err(TrackerError::invalid(
            "--input accepts only '-' (JSON from stdin)",
        ));
    }
    let mut raw = String::new();
    io::stdin()
        .read_to_string(&mut raw)
        .map_err(|_| TrackerError::invalid("could not read setup JSON from stdin"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|_| TrackerError::invalid("setup input must be valid JSON"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| TrackerError::invalid("setup input must be one JSON object"))
}

fn complete_input(
    descriptor: &ProviderDescriptor,
    mut input: SetupInput,
) -> Result<SetupInput, TrackerError> {
    if !io::stdin().is_terminal() {
        return Ok(input);
    }
    if descriptor.id.as_str() == "github-copilot" && !input.contains_key("enterprise_url") {
        let selection = Select::new()
            .with_prompt("Select GitHub deployment type")
            .items(["GitHub.com", "GitHub Enterprise"])
            .default(0)
            .interact()
            .map_err(|_| TrackerError::invalid("could not read GitHub deployment type"))?;
        match GitHubHost::from_selection(selection)? {
            GitHubHost::Public => {}
            GitHubHost::Enterprise => {
                eprint!(
                    "Enter your GitHub Enterprise URL or domain (company.ghe.com or https://company.ghe.com): "
                );
                io::stderr().flush().ok();
                let mut enterprise_url = String::new();
                io::stdin()
                    .read_line(&mut enterprise_url)
                    .map_err(|_| TrackerError::invalid("could not read setup input"))?;
                input.insert(
                    "enterprise_url".into(),
                    Value::String(enterprise_url.trim().to_owned()),
                );
            }
        }
    }
    let missing_fields = descriptor
        .setup
        .fields
        .iter()
        .filter(|field| field.required && !input.contains_key(&field.key))
        .cloned()
        .collect::<Vec<_>>();
    for field in missing_fields {
        let raw = if field.kind == SetupFieldKind::Secret {
            rpassword::prompt_password(format!("{}: ", field.label))
                .map_err(|_| TrackerError::invalid("could not read setup input"))?
        } else {
            eprint!("{}: ", field.label);
            io::stderr().flush().ok();
            let mut value = String::new();
            io::stdin()
                .read_line(&mut value)
                .map_err(|_| TrackerError::invalid("could not read setup input"))?;
            value.trim_end().to_owned()
        };
        let value = if field.kind == SetupFieldKind::Boolean {
            Value::Bool(raw.parse::<bool>().map_err(|_| {
                TrackerError::invalid("boolean setup input must be 'true' or 'false'")
                    .detail("field", field.key.clone())
            })?)
        } else {
            Value::String(raw)
        };
        input.insert(field.key.clone(), value);
    }
    Ok(input)
}

#[derive(Debug, PartialEq, Eq)]
enum GitHubHost {
    Public,
    Enterprise,
}

impl GitHubHost {
    fn from_selection(selection: usize) -> Result<Self, TrackerError> {
        match selection {
            0 => Ok(Self::Public),
            1 => Ok(Self::Enterprise),
            _ => Err(TrackerError::invalid(
                "invalid GitHub deployment type selection",
            )),
        }
    }
}

fn canonical_name(command: &Command) -> &'static str {
    match command {
        Command::Providers(ProvidersArgs {
            command: ProvidersCommand::List,
        }) => "providers.list",
        Command::Providers(ProvidersArgs {
            command: ProvidersCommand::Describe { .. },
        }) => "providers.describe",
        Command::Add(_) => "add",
        Command::List => "list",
        Command::Show { .. } => "show",
        Command::Setup(_) => "setup",
        Command::Enable { .. } => "enable",
        Command::Disable { .. } => "disable",
        Command::Remove(_) => "remove",
        Command::Usage(_) => "usage",
        Command::Debug => "debug",
    }
}

fn requested_format() -> OutputFormat {
    let args: Vec<String> = std::env::args().collect();
    for (index, arg) in args.iter().enumerate() {
        if arg == "--format" {
            if args.get(index + 1).map(String::as_str) == Some("human") {
                return OutputFormat::Human;
            }
        } else if arg == "--format=human" || arg == "--human" {
            return OutputFormat::Human;
        }
    }
    OutputFormat::Json
}

fn emit(envelope: &Envelope, format: OutputFormat, pretty: bool) -> ExitCode {
    match format {
        OutputFormat::Json => {
            let output = if pretty {
                serde_json::to_string_pretty(envelope)
            } else {
                serde_json::to_string(envelope)
            };
            match output {
                Ok(output) => println!("{output}"),
                Err(_) => println!(
                    "{{\"schema_version\":2,\"command\":\"cli\",\"ok\":false,\"partial\":false,\"data\":null,\"warnings\":[],\"errors\":[{{\"code\":\"storage_error\",\"message\":\"could not serialize response\"}}]}}"
                ),
            }
        }
        OutputFormat::Human => {
            let rendered = human::render(envelope);
            if !rendered.stdout.is_empty() {
                println!("{}", rendered.stdout);
            }
            if !rendered.stderr.is_empty() {
                eprintln!("{}", rendered.stderr);
            }
        }
    }
    ExitCode::from(envelope.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_interactive_github_deployment_choices() {
        assert_eq!(GitHubHost::from_selection(0).unwrap(), GitHubHost::Public);
        assert_eq!(
            GitHubHost::from_selection(1).unwrap(),
            GitHubHost::Enterprise
        );
        assert!(GitHubHost::from_selection(2).is_err());
    }
}
