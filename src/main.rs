// Kite CLI — webhook event intelligence for AI agents
mod commands;
mod config;
mod github;
mod manifest;
mod pipeline;
pub mod platform;
mod queue;
mod sinks;
mod skill_content;
mod ws_client;

use clap::{ArgAction, Parser, Subcommand};

const CLI_AFTER_HELP: &str = "\
Quick Reference:
  stream               --source --event-type --json --compact
  proxy                --source --target --route
  listen               --socket --source
  run                  --manifest
  retry                --source --target
  github install       --repo --events --all-events --rotate-secret
  login                --server
  endpoints create     --source
  endpoints deactivate --id
  keys create          --name --scopes --permissions --expires-at
  keys revoke          --id
  skill install        <name>
  skill list
  logs                 --limit
  update               --check --force --server

Use `kite stream --json` to print full CloudEvent payloads.
Use `kite <command> --help` for complete options.";

const GITHUB_INSTALL_AFTER_HELP: &str = "\
Examples:
  # Install the default agent-focused GitHub event bundle
  kite github install --repo owner/repo

  # Replace defaults with an explicit event list
  kite github install --repo owner/repo --events push,pull_request

  # Subscribe to every GitHub event
  kite github install --repo owner/repo --all-events

  # Rotate the stored GitHub secret while reinstalling the hook
  kite github install --repo owner/repo --rotate-secret

Default events:
  push,pull_request,issues,issue_comment,pull_request_review,pull_request_review_comment

Notes:
  --events replaces the default bundle entirely.
  --all-events is mutually exclusive with --events.
  This command uses your local `gh` authentication and updates the GitHub webhook in-place when rerun.";

const PROXY_AFTER_HELP: &str = "\
Examples:
  # Single target mode: all sources -> one local endpoint
  kite proxy --target http://localhost:3000/webhooks

  # Multi-route mode with fallback/default target
  kite proxy --route github=http://localhost:3001/github \\
             --route stripe=http://localhost:3002/stripe \\
             --target http://localhost:3000/default

  # Route-only mode (no fallback): errors + DLQ when source has no explicit route
  kite proxy --route github=http://localhost:3001/github

  # Downstream router mode: keep one local endpoint and dispatch by headers
  kite proxy --target http://localhost:3000/kite/router
  # Forwarded metadata headers include:
  #   x-kite-source, x-kite-event-id, x-kite-event-type, x-kite-team-id
  #   ce-id, ce-source, ce-type, ce-specversion, ce-time (if present)

Notes:
  Existing Kite webhook endpoints do not need to be recreated to use routes.
  --route keys are matched by source name (derived from event type, e.g. com.github.push -> github).";

/// CLI version: use KITE_VERSION env var (set by release CI from the git tag),
/// falling back to Cargo.toml version for local dev builds.
const VERSION: &str = match option_env!("KITE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(name = "kite", about = "Universal webhook adapter CLI")]
#[command(after_help = CLI_AFTER_HELP)]
#[command(version = VERSION, propagate_version = true)]
#[command(arg_required_else_help = true)]
struct Cli {
    /// Compatibility shortcut for `kite skill install <NAME>`
    #[arg(long, value_name = "NAME")]
    install_skill: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Stream webhook events to stdout
    Stream {
        /// Filter by source (e.g. "github", "stripe")
        #[arg(long)]
        source: Option<String>,
        /// Filter by event type
        #[arg(long, name = "type")]
        event_type: Option<String>,
        /// Output full CloudEvent JSON per line
        #[arg(long)]
        json: bool,
        /// Output compact one-line summaries only
        #[arg(long)]
        compact: bool,
        /// Execute a command for each event, passing event JSON on stdin
        #[arg(long)]
        exec: Option<String>,
        /// Persistent client ID for delivery cursor tracking
        #[arg(long)]
        client_id: Option<String>,
        /// Minimum importance level to deliver (low, normal, high, critical)
        #[arg(long)]
        importance: Option<String>,
    },
    /// Proxy webhook events to a local HTTP server
    #[command(after_help = PROXY_AFTER_HELP)]
    Proxy {
        /// Filter by source (e.g. "github", "stripe")
        #[arg(long)]
        source: Option<String>,
        /// Default target URL to forward events to
        #[arg(long)]
        target: Option<String>,
        /// Source-specific route in the form <source>=<target> (repeatable)
        #[arg(long, value_name = "SOURCE=TARGET", action = ArgAction::Append)]
        route: Vec<String>,
        /// Persistent client ID for delivery cursor tracking
        #[arg(long)]
        client_id: Option<String>,
    },
    /// Listen for events via Unix socket or exec
    Listen {
        /// Unix socket path to create
        #[arg(long)]
        socket: Option<String>,
        /// Filter by source
        #[arg(long)]
        source: Option<String>,
    },
    /// Run with a kite.json manifest
    Run {
        /// Path to kite.json manifest file
        #[arg(long)]
        manifest: String,
    },
    /// Retry failed events from the dead letter queue
    Retry {
        /// Only retry events from this source
        #[arg(long)]
        source: Option<String>,
        /// Target URL for retried events
        #[arg(long)]
        target: String,
    },
    /// Log in to Kite via device auth flow
    Login {
        /// Server URL
        #[arg(long, default_value = "https://getkite.sh")]
        server: String,
    },
    /// Install GitHub webhooks into repositories using local gh auth
    Github {
        #[command(subcommand)]
        command: GithubCommand,
    },
    /// Manage webhook endpoints
    Endpoints {
        #[command(subcommand)]
        command: EndpointsCommand,
    },
    /// Manage API keys
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Install and inspect agent skills
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// View persisted event logs
    Logs {
        /// Number of events to fetch
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Show dashboard-equivalent status summary
    Status,
    /// Update the installed kite binary
    Update {
        /// Check whether an update is available without installing it
        #[arg(long)]
        check: bool,
        /// Force reinstall even if current version is up to date
        #[arg(long)]
        force: bool,
        /// Override update server base URL
        #[arg(long)]
        server: Option<String>,
    },
    /// Inspect and manage the local event queue
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Send and receive agent-to-agent messages on Kite Cloud
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Register a stable agent identity on this machine
    Register {
        /// Optional friendly name (slugged into the agent id)
        #[arg(long)]
        name: Option<String>,
    },
    /// Listen for `com.kite.agent.message` events addressed to this agent
    Listen {
        /// Override the agent id to listen as (defaults to the registered id)
        #[arg(long, value_name = "AGENT_ID")]
        as_id: Option<String>,
        /// Print full CloudEvent JSON per line instead of a one-line summary
        #[arg(long)]
        json: bool,
    },
    /// Send an agent message to another agent on the same team
    Send {
        /// Recipient agent id
        #[arg(long, value_name = "AGENT_ID")]
        to: String,
        /// Sender agent id (defaults to the registered id on this machine)
        #[arg(long, value_name = "AGENT_ID")]
        from: Option<String>,
        /// Optional thread id for grouping replies
        #[arg(long, value_name = "THREAD_ID")]
        thread: Option<String>,
        /// Optional event id this message replies to
        #[arg(long, value_name = "EVENT_ID")]
        reply_to: Option<String>,
        /// Message body
        body: String,
    },
}

#[derive(Subcommand)]
enum EndpointsCommand {
    /// List endpoints
    List,
    /// Create or rotate endpoint credentials for a source (GitHub includes a one-time webhook secret)
    Create {
        /// Source name (e.g. github, stripe)
        #[arg(long)]
        source: String,
        /// GitHub repo (owner/name) — auto-registers webhook via GitHub API
        #[arg(long)]
        repo: Option<String>,
        /// Webhook events to subscribe to (comma-separated, default: push,pull_request,issues)
        #[arg(long, value_delimiter = ',')]
        events: Option<Vec<String>>,
        /// Replace existing webhook on the repo
        #[arg(long)]
        force: bool,
        /// GitHub personal access token (falls back to GITHUB_TOKEN env or gh CLI)
        #[arg(long)]
        github_token: Option<String>,
    },
    /// Deactivate endpoint by id
    Deactivate {
        /// Endpoint id
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum GithubCommand {
    /// Create or update a GitHub repository webhook that points at Kite
    #[command(after_help = GITHUB_INSTALL_AFTER_HELP)]
    Install {
        /// Repository in OWNER/REPO format
        #[arg(long)]
        repo: String,
        /// Comma-separated GitHub event names that replace the default bundle
        #[arg(long, value_delimiter = ',')]
        events: Vec<String>,
        /// Subscribe the webhook to all GitHub events
        #[arg(long, conflicts_with = "events")]
        all_events: bool,
        /// Rotate the stored GitHub webhook secret while reinstalling
        #[arg(long)]
        rotate_secret: bool,
    },
}

#[derive(Subcommand)]
enum KeysCommand {
    /// List API keys
    List,
    /// Create a new API key
    Create {
        /// Optional key name
        #[arg(long)]
        name: Option<String>,
        /// Comma-separated scopes
        #[arg(long, value_delimiter = ',')]
        scopes: Vec<String>,
        /// Comma-separated permissions
        #[arg(long, value_delimiter = ',')]
        permissions: Vec<String>,
        /// Optional RFC3339 expiry timestamp
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// Revoke API key by id
    Revoke {
        /// Key id
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Install a skill from the default registry
    Install {
        /// Skill name (e.g. weather, github)
        name: String,
    },
    /// List installed skills
    List,
    /// Export the kite SKILL.md to agent skill directories
    Export {
        /// Target platform: claude, agents, openclaw, or paperclip (default: claude)
        #[arg(long)]
        format: Option<String>,
        /// Auto-detect all present agent skill directories and export to each
        #[arg(long)]
        auto: bool,
    },
    /// Publish a skill to the central registry
    #[command(disable_version_flag = true)]
    Publish {
        /// Skill name (alphanumeric, hyphens, underscores, dots)
        #[arg(long)]
        name: String,
        /// Semver version string (e.g. "1.0.0")
        #[arg(long)]
        version: String,
        /// Short description of the skill
        #[arg(long)]
        description: Option<String>,
        /// Skill content (reads SKILL.md from current directory if omitted)
        #[arg(long)]
        content: Option<String>,
        /// Make the skill publicly discoverable
        #[arg(long)]
        public: bool,
    },
    /// Search for skills in the registry
    Search {
        /// Search query (matches name and description)
        query: Option<String>,
    },
    /// Install a skill from the remote registry by ID
    RegistryInstall {
        /// Skill ID (UUID) to install
        #[arg(long)]
        skill_id: String,
        /// Specific version ID to install (latest if omitted)
        #[arg(long)]
        version_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum QueueCommand {
    /// List queued events (with optional filters)
    List {
        /// Filter by status (pending, ready, delivered, failed, filtered, enriching)
        #[arg(long)]
        status: Option<String>,
        /// Filter by source (e.g. "github")
        #[arg(long)]
        source: Option<String>,
        /// Filter by importance (low, normal, high, critical)
        #[arg(long)]
        importance: Option<String>,
        /// Only show events created since this duration ago (e.g. "24h", "7d")
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of events to show
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show full details for a single queued event
    Show {
        /// Sequence number of the event
        seq: i64,
    },
    /// Replay queued events to a target URL
    Replay {
        /// Filter by status (default: failed)
        #[arg(long)]
        status: Option<String>,
        /// Filter by source
        #[arg(long)]
        source: Option<String>,
        /// Replay a single event by sequence number
        #[arg(long)]
        seq: Option<i64>,
        /// Target URL to POST events to
        #[arg(long)]
        target: String,
    },
    /// Delete events older than a given duration
    Flush {
        /// Delete events older than this duration (e.g. "7d", "24h", "30m")
        before: String,
    },
    /// Show event counts grouped by status
    Stats,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("kite_cli=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    if let Some(name) = cli.install_skill {
        if cli.command.is_some() {
            anyhow::bail!(
                "`--install-skill` cannot be combined with a subcommand. Use either `kite --install-skill <name>` or `kite skill install <name>`."
            );
        }
        commands::skill::install(name)?;
        return Ok(());
    }

    let Some(command) = cli.command else {
        // clap enforces arg_required_else_help; this is a defensive fallback.
        anyhow::bail!("No command provided. Run `kite --help`.");
    };

    match command {
        Commands::Stream {
            source,
            event_type,
            json,
            compact,
            exec,
            client_id,
            importance,
        } => {
            commands::stream::run(
                source, event_type, json, compact, exec, client_id, importance,
            )
            .await?;
        }
        Commands::Proxy {
            source,
            target,
            route,
            client_id,
        } => {
            commands::proxy::run(source, target, route, client_id).await?;
        }
        Commands::Listen { socket, source } => {
            if let Some(socket_path) = socket {
                commands::listen::run(socket_path, source).await?;
            } else {
                anyhow::bail!("Must specify --socket path");
            }
        }
        Commands::Run { manifest } => {
            commands::run::run(manifest).await?;
        }
        Commands::Retry { source, target } => {
            commands::retry::run(source, target).await?;
        }
        Commands::Login { server } => {
            commands::login::run(server).await?;
        }
        Commands::Github { command } => match command {
            GithubCommand::Install {
                repo,
                events,
                all_events,
                rotate_secret,
            } => {
                commands::github::install(repo, events, all_events, rotate_secret).await?;
            }
        },
        Commands::Endpoints { command } => match command {
            EndpointsCommand::List => {
                commands::endpoints::list().await?;
            }
            EndpointsCommand::Create {
                source,
                repo,
                events,
                force,
                github_token,
            } => {
                commands::endpoints::create(source, repo, events, force, github_token).await?;
            }
            EndpointsCommand::Deactivate { id } => {
                commands::endpoints::deactivate(id).await?;
            }
        },
        Commands::Keys { command } => match command {
            KeysCommand::List => {
                commands::keys::list().await?;
            }
            KeysCommand::Create {
                name,
                scopes,
                permissions,
                expires_at,
            } => {
                commands::keys::create(name, scopes, permissions, expires_at).await?;
            }
            KeysCommand::Revoke { id } => {
                commands::keys::revoke(id).await?;
            }
        },
        Commands::Skill { command } => match command {
            SkillCommand::Install { name } => {
                commands::skill::install(name)?;
            }
            SkillCommand::List => {
                commands::skill::list()?;
            }
            SkillCommand::Export { format, auto } => {
                commands::export::run(format, auto)?;
            }
            SkillCommand::Publish {
                name,
                version,
                description,
                content,
                public,
            } => {
                commands::skill_registry::publish(name, version, description, content, public)
                    .await?;
            }
            SkillCommand::Search { query } => {
                commands::skill_registry::search(query).await?;
            }
            SkillCommand::RegistryInstall {
                skill_id,
                version_id,
            } => {
                commands::skill_registry::install_from_registry(skill_id, version_id).await?;
            }
        },
        Commands::Logs { limit } => {
            commands::logs::recent(limit).await?;
        }
        Commands::Status => {
            commands::status::run().await?;
        }
        Commands::Update {
            check,
            force,
            server,
        } => {
            commands::update::run(server, check, force).await?;
        }
        Commands::Queue { command } => match command {
            QueueCommand::List {
                status,
                source,
                importance,
                since,
                limit,
            } => {
                commands::queue::list(status, source, importance, since, limit)?;
            }
            QueueCommand::Show { seq } => {
                commands::queue::show(seq)?;
            }
            QueueCommand::Replay {
                status,
                source,
                seq,
                target,
            } => {
                commands::queue::replay(status, source, seq, target).await?;
            }
            QueueCommand::Flush { before } => {
                commands::queue::flush(before)?;
            }
            QueueCommand::Stats => {
                commands::queue::stats()?;
            }
        },
        Commands::Agent { command } => match command {
            AgentCommand::Register { name } => {
                commands::agent::register(name)?;
            }
            AgentCommand::Listen { as_id, json } => {
                commands::agent::listen(as_id, json).await?;
            }
            AgentCommand::Send {
                to,
                from,
                thread,
                reply_to,
                body,
            } => {
                commands::agent::send(to, body, from, thread, reply_to).await?;
            }
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_install_subcommand() {
        let cli = Cli::try_parse_from(["kite", "skill", "install", "weather"])
            .expect("subcommand should parse");

        assert!(matches!(
            cli.command,
            Some(Commands::Skill {
                command: SkillCommand::Install { .. }
            })
        ));
    }

    #[test]
    fn parses_install_skill_alias() {
        let cli = Cli::try_parse_from(["kite", "--install-skill", "weather"]).expect("alias parse");

        assert_eq!(cli.install_skill.as_deref(), Some("weather"));
        assert!(cli.command.is_none());
    }
}
