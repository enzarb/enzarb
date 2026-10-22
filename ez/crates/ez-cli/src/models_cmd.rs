//! `ez models`: list the models an endpoint offers.

use std::io::Read;

use anyhow::{Result, bail};
use clap::Args;
use ez_core::setup_api::{self, ModelsAuth};
use zeroize::Zeroize;

#[derive(Args, Debug, Default)]
pub struct ModelsArgs {
    /// `anthropic` or `openai` (OpenAI-compatible: OpenAI, Ollama, OpenRouter, vLLM, ...).
    /// Required unless --probe is given (then it forces that API flavor).
    #[arg(long, required_unless_present = "probe")]
    pub provider: Option<String>,
    /// Endpoint (default: the provider's usual URL; required with --probe).
    #[arg(long)]
    pub base_url: Option<String>,
    /// Auto-detect the API flavor of --base-url (Anthropic Messages or OpenAI-compatible) using
    /// the key, and list its models. With --json: `{"kind","models","attempts","message"}`.
    #[arg(long)]
    pub probe: bool,
    /// Use this named key from credentials.toml.
    #[arg(long, conflicts_with = "api_key_stdin")]
    pub api_key_name: Option<String>,
    /// Read a key that is not saved yet from stdin (it is never stored).
    #[arg(long)]
    pub api_key_stdin: bool,
    /// JSON: `{"models":[{"id","displayName"}]}`.
    #[arg(long)]
    pub json: bool,
}

/// The auth choice of the flags; `stdin` supplies the secret for `--api-key-stdin`.
pub fn auth_from_args(
    args: &ModelsArgs,
    stdin: impl FnOnce() -> Result<String>,
) -> Result<Option<ModelsAuth>> {
    Ok(if args.api_key_stdin {
        let mut s = stdin()?;
        let t = s.trim().to_string();
        s.zeroize();
        if t.is_empty() {
            bail!("--api-key-stdin: no key on stdin");
        }
        Some(ModelsAuth::Secret(t))
    } else {
        args.api_key_name.clone().map(ModelsAuth::KeyName)
    })
}

pub async fn run(args: &ModelsArgs) -> Result<()> {
    let auth = auth_from_args(args, || {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    })?;
    if args.probe {
        let base = args.base_url.as_deref().unwrap_or("");
        let r = setup_api::probe(base, auth, None, args.provider.as_deref()).await?;
        if args.json {
            println!("{}", serde_json::to_string(&r)?);
        } else {
            println!("{}", r.message);
            for m in &r.models {
                println!("{}", m.id);
            }
        }
        return if r.kind.is_some() || args.json {
            Ok(())
        } else {
            bail!("no compatible API found")
        };
    }
    let models = setup_api::list_models(
        args.provider.as_deref().unwrap_or(""),
        args.base_url.as_deref().unwrap_or(""),
        auth,
        None,
    )
    .await?;
    if args.json {
        println!("{}", serde_json::json!({ "models": models }));
    } else {
        for m in &models {
            match &m.display_name {
                Some(d) => println!("{}  ({d})", m.id),
                None => println!("{}", m.id),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_choices() {
        let base = ModelsArgs {
            provider: Some("openai".into()),
            ..Default::default()
        };
        assert!(auth_from_args(&base, || unreachable!()).unwrap().is_none());
        let a = ModelsArgs {
            api_key_stdin: true,
            ..Default::default()
        };
        match auth_from_args(&a, || Ok(" sk \n".into())).unwrap() {
            Some(ModelsAuth::Secret(ref s)) => assert_eq!(s, "sk"),
            o => panic!("{o:?}"),
        }
        assert!(auth_from_args(&a, || Ok("\n".into())).is_err());
        let n = ModelsArgs {
            api_key_name: Some("k".into()),
            ..Default::default()
        };
        assert!(matches!(
            auth_from_args(&n, || unreachable!()).unwrap(),
            Some(ModelsAuth::KeyName(_))
        ));
    }
}
