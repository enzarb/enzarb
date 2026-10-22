//! `ez sessions`: list, show and search the project's transcripts. With `--json` the output is
//! one line of the camelCase JSON of `ez_core::transcript_api` (used by the desktop app).

use anyhow::Result;
use clap::{Args, Subcommand};
use ez_core::transcript_api::{self, ReadOptions};

use crate::{Cli, one_line};

#[derive(Args, Debug, Default)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub action: Option<SessionsAction>,
    /// Print JSON (one line). List: `[{id,createdAt,updatedAt,firstPrompt,mode,model,turns,
    /// toolCalls,inputTokens,outputTokens,durationMs,sizeBytes,recordCount,skippedLines}]`.
    #[arg(long, global = true)]
    pub json: bool,
    /// List only sessions whose id, first prompt, mode or model contains this text.
    #[arg(long)]
    pub query: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum SessionsAction {
    /// Show one session's records (JSON: `{meta,records,total,offset,limit,skippedLines}`).
    Show {
        id: String,
        /// First record to return.
        #[arg(long)]
        offset: Option<usize>,
        /// Records to return (default 500, at most 5000).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Search all sessions (JSON: `[{id,recordIndex,type,snippet}]`).
    Search {
        text: String,
        /// Most hits to return (default 50).
        #[arg(long)]
        limit: Option<usize>,
    },
}

/// Handles the JSON forms and `show`/`search`; returns false for the plain `ez sessions` listing,
/// which still goes through the agent.
pub fn run(cli: &Cli, args: &SessionsArgs) -> Result<bool> {
    let project = cli.project_dir()?;
    match &args.action {
        Some(SessionsAction::Show { id, offset, limit }) => {
            let r = transcript_api::read(
                &project,
                id,
                ReadOptions {
                    offset: *offset,
                    limit: *limit,
                },
            )?;
            if args.json {
                println!("{}", serde_json::to_string(&r)?);
            } else {
                let m = &r.meta.summary;
                println!("{}  {}  {}  {}", m.id, m.mode, m.model, m.created_at);
                for rec in &r.records {
                    println!(
                        "{:>5}  {:<17} {}",
                        rec["index"],
                        rec["type"].as_str().unwrap_or(""),
                        one_line(&brief(rec), 100)
                    );
                }
                if r.offset + r.records.len() < r.total {
                    println!("... {} records in total", r.total);
                }
            }
        }
        Some(SessionsAction::Search { text, limit }) => {
            let hits = transcript_api::search(&project, text, *limit)?;
            if args.json {
                println!("{}", serde_json::to_string(&hits)?);
            } else if hits.is_empty() {
                println!("no matches");
            } else {
                for h in hits {
                    println!(
                        "{}  #{:<5} {:<17} {}",
                        h.id, h.record_index, h.record_type, h.snippet
                    );
                }
            }
        }
        None if args.json => {
            let list = transcript_api::list(&project, args.query.as_deref())?;
            println!("{}", serde_json::to_string(&list)?);
        }
        None => return Ok(false),
    }
    Ok(true)
}

/// A short text for a normalized record, for the plain `show` output.
fn brief(rec: &serde_json::Value) -> String {
    let s = |k: &str| rec[k].as_str().unwrap_or("").to_string();
    match rec["type"].as_str().unwrap_or("") {
        "user_message" | "assistant_message" => rec["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default(),
        "tool_call" => s("title"),
        "tool_result" => format!("{}: {}", s("name"), s("output")),
        "hook_run" => s("command"),
        "mode_change" => format!("{} -> {}", s("from"), s("to")),
        "compaction" => s("summary"),
        _ => String::new(),
    }
}
