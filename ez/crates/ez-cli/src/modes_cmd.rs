//! `ez modes`: read, validate and write `.ez/modes.toml` as JSON (one line, camelCase; see
//! `ez_core::modes_api`). Used by the desktop app inside WSL; the document goes in on stdin.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use ez_core::modes_api::{self, ModesDoc};

use crate::Cli;

#[derive(Subcommand, Debug)]
pub enum ModesAction {
    /// Print the modes document and the editor catalog: `{status,path,error?,defaultMode?,modes,
    /// globalHooks?,catalog}` (status: missing, broken or ready).
    Get {
        #[arg(long)]
        json: bool,
    },
    /// Check a document from stdin (`{defaultMode,modes:[...]}`): `[{path,message,severity}]`.
    Validate {
        #[arg(long)]
        doc_stdin: bool,
        #[arg(long)]
        json: bool,
    },
    /// Write a document from stdin to modes.toml, preserving comments and formatting of the parts
    /// that did not change: `{path,changed}`. With --preview nothing is written and the result is
    /// `{diff,changed,rewritten}`.
    Set {
        #[arg(long)]
        doc_stdin: bool,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        json: bool,
    },
}

fn read_doc() -> Result<ModesDoc> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    serde_json::from_str(&text).context("--doc-stdin: invalid modes document JSON")
}

fn out<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string(v)?);
    Ok(())
}

pub async fn run(cli: &Cli, action: &ModesAction) -> Result<()> {
    let project = cli.project_dir()?;
    match action {
        ModesAction::Get { .. } => out(&modes_api::get(&project, None).await),
        ModesAction::Validate { doc_stdin, .. } => {
            if !doc_stdin {
                bail!("validate needs --doc-stdin");
            }
            out(&modes_api::validate(&project, None, &read_doc()?))
        }
        ModesAction::Set {
            doc_stdin, preview, ..
        } => {
            if !doc_stdin {
                bail!("set needs --doc-stdin");
            }
            let doc = read_doc()?;
            if *preview {
                out(&modes_api::preview(&project, None, &doc)?)
            } else {
                out(&modes_api::apply(&project, None, &doc)?)
            }
        }
    }
}
