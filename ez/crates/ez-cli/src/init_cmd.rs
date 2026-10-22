//! `ez init`: write `.ez/modes.toml` from the embedded templates.

use anyhow::{Context, Result, bail};
use ez_core::init::{self, InitOptions};

use crate::Cli;

pub async fn run(
    cli: &Cli,
    minimal: bool,
    modes: &[String],
    diff: bool,
    force: bool,
    json: bool,
) -> Result<()> {
    let dir = cli.project_dir()?;
    if json && !diff {
        let r = ez_core::setup_api::init_modes(
            &dir,
            &ez_core::setup_api::InitInput {
                minimal,
                modes: modes.to_vec(),
                force,
            },
        )
        .await?;
        println!("{}", serde_json::to_string(&r)?);
        return Ok(());
    }
    let tasks = ez_mise::discover(&dir).await;
    let opts = InitOptions {
        minimal,
        only: if modes.is_empty() {
            None
        } else {
            Some(modes.to_vec())
        },
    };
    let generated = init::generate(&tasks, &opts)?;
    let path = dir.join(".ez").join("modes.toml");

    if diff {
        let existing = std::fs::read_to_string(&path)
            .with_context(|| format!("no existing {} to compare with", path.display()))?;
        let d = init::diff(&existing, &generated);
        if d.trim().is_empty() {
            println!("{} is up to date", path.display());
        } else {
            print!("{d}");
        }
        return Ok(());
    }

    if path.exists() && !force {
        bail!(
            "{} already exists; use --force to overwrite or --diff to compare",
            path.display()
        );
    }
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(&path, generated)?;
    println!(
        "wrote {} ({} mise task(s) discovered)",
        path.display(),
        tasks.len()
    );
    println!(
        "note: .ez/sessions holds session transcripts and is meant to be committed \
         alongside your code; do not add it to .gitignore."
    );
    Ok(())
}
