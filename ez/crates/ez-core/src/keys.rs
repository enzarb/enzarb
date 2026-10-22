//! Named API keys in `credentials.toml` (`[keys.<name>] api_key`, optional `description`,
//! `created_at`). Profiles reference a key by name via `api_key_name`; the secret itself is
//! only ever written to this file (mode 0600).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::config::{credentials_path, user_config_dir};

#[derive(Debug, Clone, PartialEq)]
pub struct KeyInfo {
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
    /// Masked secret for display: `…` plus the last four characters.
    pub masked: String,
}

pub fn mask(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if secret.chars().count() <= 8 {
        "…".into()
    } else {
        format!("…{tail}")
    }
}

pub fn validate_key_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("invalid key name `{name}`: use letters, digits, `-`, `_` or `.`");
    }
    Ok(())
}

fn path(user_dir: Option<&Path>) -> Result<PathBuf> {
    credentials_path(user_dir).context("cannot determine the user config directory")
}

fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Keys in `credentials.toml`, sorted by name. Secrets are masked.
pub fn list_keys(user_dir: Option<&Path>) -> Result<Vec<KeyInfo>> {
    let text = read(&path(user_dir)?)?;
    let t: toml::Table = text.parse().context("parsing credentials.toml")?;
    let mut out = vec![];
    if let Some(toml::Value::Table(keys)) = t.get("keys") {
        for (name, v) in keys {
            out.push(KeyInfo {
                name: name.clone(),
                description: v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from),
                created_at: v
                    .get("created_at")
                    .and_then(|d| d.as_str())
                    .map(String::from),
                masked: mask(v.get("api_key").and_then(|k| k.as_str()).unwrap_or("")),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `credentials.toml` text with the key stored.
pub fn render_key(
    existing: &str,
    name: &str,
    secret: &str,
    description: Option<&str>,
) -> Result<String> {
    validate_key_name(name)?;
    if secret.trim().is_empty() {
        bail!("the key is empty");
    }
    let mut doc: DocumentMut = existing.parse().context("parsing credentials.toml")?;
    if !doc.contains_key("keys") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("keys", Item::Table(t));
    }
    let keys = doc["keys"]
        .as_table_mut()
        .context("`keys` must be a table")?;
    let mut t = Table::new();
    t["api_key"] = value(secret.trim());
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        t["description"] = value(d);
    }
    let prev_created = keys
        .get(name)
        .and_then(|k| k.get("created_at"))
        .and_then(|c| c.as_str())
        .map(String::from);
    t["created_at"] = value(
        prev_created.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    );
    keys.insert(name, Item::Table(t));
    Ok(doc.to_string())
}

/// Write `contents` to `path` with owner-only permissions (0600 on unix), atomically.
pub(crate) fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("credentials path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(".credentials.toml.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("writing {}", path.display()))
}

/// Add or replace a named key.
pub fn add_key(
    user_dir: Option<&Path>,
    name: &str,
    secret: &str,
    description: Option<&str>,
) -> Result<PathBuf> {
    let p = path(user_dir)?;
    let text = render_key(&read(&p)?, name, secret, description)?;
    write_private(&p, &text)?;
    Ok(p)
}

/// Remove a named key; `Ok(false)` if it did not exist.
pub fn remove_key(user_dir: Option<&Path>, name: &str) -> Result<bool> {
    let p = path(user_dir)?;
    let mut doc: DocumentMut = read(&p)?.parse().context("parsing credentials.toml")?;
    let removed = doc
        .get_mut("keys")
        .and_then(Item::as_table_mut)
        .map(|k| k.remove(name).is_some())
        .unwrap_or(false);
    if removed {
        write_private(&p, &doc.to_string())?;
    }
    Ok(removed)
}

/// Where a profile's `api_key_name` lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyScope {
    /// The user's `config.toml` (all projects).
    User,
    /// This project's `.ez/config.toml` (this project only).
    Project,
}

/// `config.toml` text with `[profiles.<profile>] api_key_name = name` (or removed for `None`).
pub fn render_key_ref(existing: &str, profile: &str, name: Option<&str>) -> Result<String> {
    let mut doc: DocumentMut = existing.parse().context("parsing config.toml")?;
    match name {
        Some(n) => {
            validate_key_name(n)?;
            for k in ["profiles", profile] {
                let parent: &mut Table = if k == "profiles" {
                    doc.as_table_mut()
                } else {
                    doc["profiles"]
                        .as_table_mut()
                        .context("`profiles` must be a table")?
                };
                if !parent.contains_key(k) {
                    let mut t = Table::new();
                    t.set_implicit(true);
                    parent.insert(k, Item::Table(t));
                }
            }
            let t = doc["profiles"][profile]
                .as_table_mut()
                .context("profile must be a table")?;
            if t.get("api_key_name").and_then(Item::as_str) != Some(n) {
                t["api_key_name"] = value(n);
            }
        }
        None => {
            if let Some(t) = doc
                .get_mut("profiles")
                .and_then(|p| p.get_mut(profile))
                .and_then(Item::as_table_mut)
            {
                t.remove("api_key_name");
            }
        }
    }
    Ok(doc.to_string())
}

fn config_path(scope: KeyScope, user_dir: Option<&Path>, project: &Path) -> Result<PathBuf> {
    Ok(match scope {
        KeyScope::User => user_config_dir(user_dir)
            .context("cannot determine the user config directory")?
            .join("config.toml"),
        KeyScope::Project => project.join(".ez").join("config.toml"),
    })
}

/// Point `profile` at named key `name` in the user or project config.
pub fn use_key(
    name: &str,
    profile: &str,
    scope: KeyScope,
    user_dir: Option<&Path>,
    project: &Path,
) -> Result<PathBuf> {
    let p = config_path(scope, user_dir, project)?;
    let text = render_key_ref(&read(&p)?, profile, Some(name))?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))?;
    Ok(p)
}

/// Every `api_key_name` reference in the user and project configs:
/// `(key name, "profile `p` (user)" / "profile `p` (this project)")`.
pub fn key_references(user_dir: Option<&Path>, project: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    for (scope, label) in [
        (KeyScope::User, "user"),
        (KeyScope::Project, "this project"),
    ] {
        let Ok(p) = config_path(scope, user_dir, project) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let Ok(t) = text.parse::<toml::Table>() else {
            continue;
        };
        if let Some(toml::Value::Table(profiles)) = t.get("profiles") {
            for (pn, v) in profiles {
                if let Some(k) = v.get("api_key_name").and_then(|k| k.as_str()) {
                    out.push((k.to_string(), format!("profile `{pn}` ({label})")));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_remove_and_mask() {
        let d = tempfile::tempdir().unwrap();
        add_key(
            Some(d.path()),
            "work",
            "sk-ant-abcdefgh1234",
            Some("work key"),
        )
        .unwrap();
        add_key(Some(d.path()), "acme", "short", None).unwrap();
        let l = list_keys(Some(d.path())).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "acme");
        assert_eq!(l[0].masked, "…");
        assert_eq!(l[1].masked, "…1234");
        assert_eq!(l[1].description.as_deref(), Some("work key"));
        assert!(l[1].created_at.is_some());
        assert_eq!(
            crate::config::named_key(Some(d.path()), "work").as_deref(),
            Some("sk-ant-abcdefgh1234")
        );
        assert!(remove_key(Some(d.path()), "work").unwrap());
        assert!(!remove_key(Some(d.path()), "work").unwrap());
        assert_eq!(list_keys(Some(d.path())).unwrap().len(), 1);
        assert!(add_key(Some(d.path()), "bad name", "x", None).is_err());
    }

    #[test]
    fn key_ref_render() {
        let t = render_key_ref("# c\n[profiles.a]\nmodel = \"m\"\n", "a", Some("k1")).unwrap();
        assert!(
            t.contains("# c") && t.contains("api_key_name = \"k1\""),
            "{t}"
        );
        let t = render_key_ref("", "b", Some("k")).unwrap();
        assert!(t.contains("[profiles.b]"), "{t}");
        let t2 = render_key_ref(&t, "b", None).unwrap();
        assert!(!t2.contains("api_key_name"));
    }
}
