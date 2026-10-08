//! Command line: `migrate [--require-roles]` and `run --roles reducer,executor,ticker`.

use std::collections::BTreeSet;
use std::str::FromStr;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "enzarb-workflows", version, about = "enzarb workflow engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Apply database migrations and exit (run as the schema owner role).
    Migrate {
        /// Fail if the runtime roles (enzarb_connectors, enzarb_ui, enzarb_workflows) do not
        /// exist, instead of skipping their grants.
        #[arg(long)]
        require_roles: bool,
    },
    /// Run one or more engine roles until SIGTERM/SIGINT.
    Run {
        /// Comma-separated roles: reducer, executor, ticker.
        #[arg(long, env = "ENZARB_ROLES", value_delimiter = ',', required = true)]
        roles: Vec<Role>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Reducer,
    Executor,
    Ticker,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reducer => "reducer",
            Role::Executor => "executor",
            Role::Ticker => "ticker",
        }
    }
}

impl FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "reducer" => Ok(Role::Reducer),
            "executor" => Ok(Role::Executor),
            "ticker" => Ok(Role::Ticker),
            other => Err(format!(
                "unknown role `{other}` (expected reducer, executor or ticker)"
            )),
        }
    }
}

/// De-duplicated, ordered role set.
pub fn role_set(roles: &[Role]) -> BTreeSet<Role> {
    roles.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roles() {
        let cli = Cli::try_parse_from(["x", "run", "--roles", "reducer,Executor, ticker"]).unwrap();
        let Command::Run { roles } = cli.command else {
            panic!()
        };
        assert_eq!(
            role_set(&roles).into_iter().collect::<Vec<_>>(),
            vec![Role::Reducer, Role::Executor, Role::Ticker]
        );
    }

    #[test]
    fn dedupes_roles() {
        let cli = Cli::try_parse_from(["x", "run", "--roles", "ticker,ticker"]).unwrap();
        let Command::Run { roles } = cli.command else {
            panic!()
        };
        assert_eq!(role_set(&roles).len(), 1);
    }

    #[test]
    fn rejects_unknown_role() {
        let err = Cli::try_parse_from(["x", "run", "--roles", "reducer,bogus"]).unwrap_err();
        assert!(err.to_string().contains("unknown role `bogus`"), "{err}");
    }

    #[test]
    fn run_needs_roles_and_migrate_flag_parses() {
        assert!(Cli::try_parse_from(["x", "run"]).is_err());
        let cli = Cli::try_parse_from(["x", "migrate", "--require-roles"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Migrate {
                require_roles: true
            }
        ));
    }
}
