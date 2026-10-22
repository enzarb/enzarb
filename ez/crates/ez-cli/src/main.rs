//! `ez`: the ez command line. An ACP client: the agent is only ever reached over ACP.

mod client;
mod headless;
mod init_cmd;
mod keys_cmd;
mod mcp_cmd;
mod models_cmd;
mod modes_cmd;
mod sessions_cmd;
mod setup_cmd;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ez_core::{Agent, AgentOptions};

#[derive(Parser, Debug)]
#[command(name = "ez", version = env!("EZ_BUILD_VERSION"), about = "ez: a mode-driven coding agent")]
pub struct Cli {
    /// Initial prompt (interactive) or the prompt to run (with --print).
    pub prompt: Option<String>,

    /// Headless mode: print the response and exit.
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Output format for --print.
    #[arg(long, value_enum, default_value = "text")]
    pub output_format: OutputFormat,

    /// Tools to auto-allow in --print mode (comma separated, e.g. Bash,Edit).
    #[arg(long, value_delimiter = ',')]
    pub allow: Vec<String>,

    /// Allow every permission request in --print mode.
    #[arg(long)]
    pub yolo: bool,

    /// Initial mode id.
    #[arg(long)]
    pub mode: Option<String>,

    /// Model profile from config.
    #[arg(long)]
    pub profile: Option<String>,

    /// Override the model of the profile.
    #[arg(long)]
    pub model: Option<String>,

    /// Project directory (default: current directory).
    #[arg(long, global = true)]
    pub cwd: Option<PathBuf>,

    /// Resume a session by id (`YYYYMMDD-HHMMSS-xxxxxx`); without an id, pick one
    /// interactively. A value that is not a session id is treated as the prompt.
    #[arg(short = 'r', long, num_args = 0..=1, default_missing_value = "", value_name = "ID")]
    pub resume: Option<String>,

    /// Continue the most recent session.
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    StreamJson,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve the agent over ACP on stdio (for editors such as Zed).
    Acp,
    /// Generate .ez/modes.toml for this project.
    Init {
        /// Use the minimal plan/build/verify modes.
        #[arg(long)]
        minimal: bool,
        /// Comma separated subset of modes to generate.
        #[arg(long, value_delimiter = ',')]
        modes: Vec<String>,
        /// Print a diff between the existing file and what would be generated.
        #[arg(long)]
        diff: bool,
        /// Overwrite an existing modes.toml.
        #[arg(long)]
        force: bool,
        /// Print the result as JSON (`path`, `tasksDiscovered`, `modes`).
        #[arg(long)]
        json: bool,
    },
    /// Interactive setup wizard: provider, endpoint, key, models and per-mode models.
    Setup(setup_cmd::SetupArgs),
    /// Manage named API keys in credentials.toml.
    Keys {
        #[command(subcommand)]
        action: keys_cmd::KeysAction,
    },
    /// Read, validate and write .ez/modes.toml as JSON (the desktop app's mode editor).
    Modes {
        #[command(subcommand)]
        action: modes_cmd::ModesAction,
    },
    /// List the models an endpoint offers (JSON with --json).
    Models(models_cmd::ModelsArgs),
    /// List sessions of this project.
    Sessions(sessions_cmd::SessionsArgs),
    /// Inspect configured MCP servers.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum McpAction {
    /// Connect to every configured MCP server and print its status and tools.
    List,
}

/// True for ids produced by `new_session_id`: `YYYYMMDD-HHMMSS-<6 hex>`.
pub fn looks_like_session_id(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    p.len() == 3
        && p[0].len() == 8
        && p[1].len() == 6
        && p[2].len() == 6
        && p[0].bytes().chain(p[1].bytes()).all(|b| b.is_ascii_digit())
        && p[2].bytes().all(|b| b.is_ascii_hexdigit())
}

impl Cli {
    /// `ez -r "some prompt"`: clap binds the prompt to `--resume`; if the value
    /// is not a session id, move it to the prompt and fall back to the picker.
    pub fn normalize(mut self) -> Self {
        if let Some(v) = self.resume.take() {
            if v.is_empty() || looks_like_session_id(&v) {
                self.resume = Some(v);
            } else {
                self.prompt = Some(match self.prompt.take() {
                    Some(rest) => format!("{v} {rest}"),
                    None => v,
                });
                self.resume = Some(String::new());
            }
        }
        self
    }

    pub fn project_dir(&self) -> Result<PathBuf> {
        let dir = match &self.cwd {
            Some(d) => d.clone(),
            None => std::env::current_dir()?,
        };
        dir.canonicalize()
            .with_context(|| format!("cannot access {}", dir.display()))
    }

    pub fn build_agent(&self) -> Arc<Agent> {
        Arc::new(Agent::new(AgentOptions {
            profile_override: self.profile.clone(),
            model_override: self.model.clone(),
            ..Default::default()
        }))
    }
}

fn init_logging(to_file: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("EZ_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    if to_file {
        let path = dirs::cache_dir().map(|d| d.join("ez").join("ez.log"));
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .try_init();
                return;
            }
        }
        // No usable log file: stay silent rather than corrupt the screen.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("off"))
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse().normalize();
    let interactive = (cli.command.is_none() && !cli.print)
        || matches!(&cli.command, Some(Command::Setup(a)) if a.interactive());
    init_logging(interactive);
    let result: Result<u8> = match &cli.command {
        Some(Command::Acp) => run_acp(&cli).await.map(|_| 0),
        Some(Command::Init {
            minimal,
            modes,
            diff,
            force,
            json,
        }) => init_cmd::run(&cli, *minimal, modes, *diff, *force, *json)
            .await
            .map(|_| 0),
        Some(Command::Setup(args)) => setup_cmd::run(&cli, args).await.map(|_| 0),
        Some(Command::Models(args)) => models_cmd::run(args).await.map(|_| 0),
        Some(Command::Modes { action }) => modes_cmd::run(&cli, action).await.map(|_| 0),
        Some(Command::Keys { action }) => keys_cmd::run(&cli, action).await.map(|_| 0),
        Some(Command::Sessions(args)) => {
            async {
                if !sessions_cmd::run(&cli, args)? {
                    run_sessions(&cli).await?;
                }
                Ok(0)
            }
            .await
        }
        Some(Command::Mcp {
            action: McpAction::List,
        }) => mcp_cmd::list(&cli).await.map(|_| 0),
        None if cli.print => headless::run(&cli).await,
        None => tui::run(&cli).await.map(|_| 0),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("ez: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run_acp(cli: &Cli) -> Result<()> {
    ez_acp::agent::serve_stdio(cli.build_agent()).await
}

/// Connect an in-process agent with a client handler and run the ACP handshake.
pub async fn connect(cli: &Cli, handler: Arc<dyn ez_acp::Handler>) -> Result<ez_acp::Connection> {
    let conn = ez_acp::agent::spawn_in_process(cli.build_agent(), handler);
    client::initialize(&conn).await?;
    Ok(conn)
}

struct NoHandler;

#[async_trait::async_trait]
impl ez_acp::Handler for NoHandler {
    async fn request(
        &self,
        method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, ez_acp::RpcError> {
        Err(ez_acp::RpcError::method_not_found(method))
    }
    async fn notification(&self, _method: &str, _params: serde_json::Value) {}
}

async fn run_sessions(cli: &Cli) -> Result<()> {
    let cwd = cli.project_dir()?;
    let conn = connect(cli, Arc::new(NoHandler)).await?;
    let sessions = client::list_sessions(&conn, &cwd).await?;
    if sessions.is_empty() {
        println!("no sessions in {}", cwd.display());
        return Ok(());
    }
    for s in sessions {
        println!(
            "{}  {}  {:<10} {}",
            s.id,
            s.updated_at,
            s.mode,
            one_line(&s.first_prompt, 60)
        );
    }
    Ok(())
}

pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let t: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("ez").chain(args.iter().copied()))
            .unwrap()
            .normalize()
    }

    #[test]
    fn session_id_shape() {
        assert!(looks_like_session_id("20250102-030405-a1b2c3"));
        assert!(!looks_like_session_id("fix the bug"));
        assert!(!looks_like_session_id("20250102-030405-zzzzzz"));
        assert!(!looks_like_session_id("2025-0102-030405"));
    }

    #[test]
    fn resume_with_id() {
        let c = parse(&["-r", "20250102-030405-a1b2c3"]);
        assert_eq!(c.resume.as_deref(), Some("20250102-030405-a1b2c3"));
        assert_eq!(c.prompt, None);
        let c = parse(&["--resume=20250102-030405-a1b2c3"]);
        assert_eq!(c.resume.as_deref(), Some("20250102-030405-a1b2c3"));
    }

    #[test]
    fn resume_bare_opens_picker() {
        let c = parse(&["-r"]);
        assert_eq!(c.resume.as_deref(), Some(""));
        assert_eq!(c.prompt, None);
    }

    #[test]
    fn resume_value_that_is_a_prompt() {
        let c = parse(&["-r", "some prompt"]);
        assert_eq!(c.resume.as_deref(), Some(""));
        assert_eq!(c.prompt.as_deref(), Some("some prompt"));
        let c = parse(&["-r", "fix", "the bug"]);
        assert_eq!(c.resume.as_deref(), Some(""));
        assert_eq!(c.prompt.as_deref(), Some("fix the bug"));
    }

    #[test]
    fn no_resume() {
        let c = parse(&["hello"]);
        assert_eq!(c.resume, None);
        assert_eq!(c.prompt.as_deref(), Some("hello"));
    }
}
