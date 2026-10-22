//! `ez setup`: the interactive wizard, or `--non-interactive` for scripts.

use anyhow::{Context, Result, bail};
use clap::Args;
use ez_core::config::Profile;
use ez_core::config::ProviderKind;
use ez_core::keys::{self, KeyScope};
use ez_core::setup::{self, NewKey, SetupPlan};
use ez_core::setup_api;
use zeroize::Zeroize;

use crate::Cli;

#[derive(Args, Debug, Default)]
pub struct SetupArgs {
    /// Skip the wizard and apply the flags below.
    #[arg(long)]
    pub non_interactive: bool,
    /// API flavor: `anthropic` or `openai` (OpenAI-compatible: OpenAI, OpenRouter, Ollama, LM
    /// Studio, vLLM, ...).
    #[arg(long)]
    pub provider: Option<String>,
    /// Endpoint URL. Required for `openai`; `anthropic` defaults to https://api.anthropic.com.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Use this existing named key from your per-user credentials.toml (or, with
    /// --api-key-stdin, create it; default name: the profile name).
    #[arg(long)]
    pub api_key_name: Option<String>,
    /// Read a new API key from stdin and store it in your per-user credentials.toml. Required
    /// unless --api-key-name names an existing key or the endpoint is local/LAN.
    #[arg(long)]
    pub api_key_stdin: bool,
    /// Save the key reference in this project's .ez/config.toml instead of your profile.
    #[arg(long)]
    pub project_key: bool,
    /// Profile name (default: the provider).
    #[arg(long)]
    pub profile: Option<String>,
    /// Comma separated models to make available (required).
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,
    /// Default model of the profile (default: the first of --models).
    #[arg(long)]
    pub default_model: Option<String>,
    /// `MODE=MODEL` to use MODEL (of this profile) in a mode, or `MODE=default`. Repeatable.
    #[arg(long = "mode-model", value_name = "MODE=MODEL")]
    pub mode_model: Vec<String>,
    /// Do not make this the default profile.
    #[arg(long)]
    pub no_default: bool,
    /// Print the current setup state as JSON (profiles, named keys, env var presence, modes) and
    /// exit. Never contains a secret.
    #[arg(long)]
    pub json_state: bool,
    /// Read the whole plan as JSON from stdin (see docs: `PlanInput`), including a new key's
    /// secret, and apply it. Implies --non-interactive.
    #[arg(long)]
    pub plan_stdin: bool,
    /// With --non-interactive/--plan-stdin: print the diff of what would be written and write
    /// nothing.
    #[arg(long)]
    pub preview: bool,
    /// With --non-interactive/--plan-stdin: print the report (or, with --preview, the diff) as JSON.
    #[arg(long)]
    pub json: bool,
}

impl SetupArgs {
    /// True when `ez setup` opens the terminal wizard.
    pub fn interactive(&self) -> bool {
        !(self.non_interactive || self.json_state || self.plan_stdin)
    }
}

pub async fn run(cli: &Cli, args: &SetupArgs) -> Result<()> {
    if args.interactive() {
        return crate::tui::setup::runner::standalone(cli).await;
    }
    let project = cli.project_dir()?;
    if args.json_state {
        let st = setup_api::state(&project, None);
        println!("{}", serde_json::to_string(&st)?);
        return Ok(());
    }
    let read_stdin = || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    };
    let mut plan = if args.plan_stdin {
        let mut text = read_stdin()?;
        let parsed = serde_json::from_str::<setup_api::PlanInput>(&text);
        text.zeroize();
        parsed
            .context("--plan-stdin: invalid plan JSON")?
            .into_plan()?
    } else {
        build_plan(args, read_stdin)?
    };
    let result = run_plan(args, &plan, &project);
    setup_api::wipe_plan(&mut plan);
    result
}

fn run_plan(args: &SetupArgs, plan: &SetupPlan, project: &std::path::Path) -> Result<()> {
    if args.preview {
        let diff = setup::preview(plan, None, project)?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string(&setup_api::PreviewView { diff })?
            );
        } else {
            print!("{diff}");
        }
        return Ok(());
    }
    let report = setup::apply(plan, None, project)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string(&setup_api::ReportView::from(&report))?
        );
    } else {
        println!("{}", crate::tui::setup::runner::summary(&report));
    }
    Ok(())
}

/// Build the plan for `--non-interactive`; `stdin` supplies the secret for `--api-key-stdin`.
pub fn build_plan(args: &SetupArgs, stdin: impl FnOnce() -> Result<String>) -> Result<SetupPlan> {
    let id = args
        .provider
        .as_deref()
        .context("--provider is required with --non-interactive")?;
    let kind = match id {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::Openai,
        _ => bail!("unknown provider `{id}`: use anthropic or openai (OpenAI-compatible)"),
    };
    if args.models.is_empty() {
        bail!("--models is required with --non-interactive");
    }
    let raw_url = match (&args.base_url, kind) {
        (Some(u), _) => u.clone(),
        (None, ProviderKind::Anthropic) => setup::ANTHROPIC_BASE_URL.to_string(),
        (None, _) => bail!(
            "--base-url is required for --provider openai, e.g. https://api.openai.com/v1 or http://localhost:11434/v1"
        ),
    };
    let base =
        setup::validate_endpoint(&raw_url).map_err(|e| anyhow::anyhow!("--base-url: {e}"))?;
    let name = args
        .profile
        .clone()
        .unwrap_or_else(|| setup::suggested_profile_name(kind, &base));
    let default_model = args
        .default_model
        .clone()
        .unwrap_or_else(|| args.models[0].clone());
    let mut models = args.models.clone();
    if !models.contains(&default_model) {
        models.insert(0, default_model.clone());
    }

    let (mut api_key_name, mut new_key) = (None, None);
    if args.api_key_stdin {
        let key_name = args.api_key_name.clone().unwrap_or_else(|| name.clone());
        keys::validate_key_name(&key_name)?;
        let secret = stdin()?.trim().to_string();
        if secret.is_empty() {
            bail!("--api-key-stdin: no key on stdin");
        }
        new_key = Some(NewKey {
            name: key_name.clone(),
            secret,
            description: Some(format!("{name} key")),
        });
        api_key_name = Some(key_name);
    } else if let Some(n) = &args.api_key_name {
        keys::validate_key_name(n)?;
        if !keys::list_keys(None)?.iter().any(|k| k.name == *n) {
            bail!(
                "named key `{n}` not found in credentials.toml; add it with `ez keys add {n}` or pass --api-key-stdin"
            );
        }
        api_key_name = Some(n.clone());
    } else if !setup::is_local_endpoint(&base) {
        bail!(
            "an API key is required for {base}: pass --api-key-stdin (stores a new key in your per-user credentials.toml) or --api-key-name <existing key>"
        );
    }

    let mut mode_models = vec![];
    for spec in &args.mode_model {
        let (mode, model) = spec
            .split_once('=')
            .with_context(|| format!("--mode-model `{spec}`: expected MODE=MODEL"))?;
        let choice = (model != "default").then(|| (name.clone(), model.to_string()));
        mode_models.push((mode.to_string(), choice));
    }

    Ok(SetupPlan {
        profile_name: name.clone(),
        profile: Profile {
            name,
            provider: kind,
            model: default_model,
            models,
            api_key_name,
            api_key_env: None,
            base_url: setup::stored_base_url(kind, &base),
            max_tokens: 32_000,
            context_window: 200_000,
            thinking: None,
        },
        set_default: !args.no_default,
        new_key,
        key_scope: if args.project_key {
            KeyScope::Project
        } else {
            KeyScope::User
        },
        mode_models,
    })
}
