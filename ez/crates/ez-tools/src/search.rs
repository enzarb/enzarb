use crate::{Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use async_trait::async_trait;
use globset::{Glob as GlobPattern, GlobMatcher};
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_RESULTS: usize = 500;

fn walk(root: &Path) -> impl Iterator<Item = PathBuf> {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
}

fn matcher(pattern: &str) -> anyhow::Result<GlobMatcher> {
    Ok(GlobPattern::new(pattern)?.compile_matcher())
}

fn rel<'a>(root: &Path, p: &'a Path) -> &'a Path {
    p.strip_prefix(root).unwrap_or(p)
}

fn glob_match(m: &GlobMatcher, root: &Path, p: &Path) -> bool {
    let r = rel(root, p);
    m.is_match(r) || r.file_name().is_some_and(|n| m.is_match(n))
}

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> String {
        "Find files by glob pattern (e.g. `**/*.rs`), respecting .gitignore. Results are sorted by modification time, newest first."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string", "description": "Directory to search (default: project root)"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn title(&self, input: &Value) -> String {
        format!("Glob {}", input["pattern"].as_str().unwrap_or("?"))
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let m = matcher(str_arg(&input, "pattern")?)?;
        let root = input["path"]
            .as_str()
            .map(|p| ctx.resolve(p))
            .unwrap_or_else(|| ctx.cwd.clone());
        let found = tokio::task::spawn_blocking(move || {
            let mut v: Vec<(std::time::SystemTime, PathBuf)> = walk(&root)
                .filter(|p| glob_match(&m, &root, p))
                .map(|p| {
                    let t = p
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    (t, p)
                })
                .collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.0));
            v.into_iter().map(|(_, p)| p).collect::<Vec<_>>()
        })
        .await?;
        if found.is_empty() {
            return Ok(ToolOutput::ok("No files found"));
        }
        let total = found.len();
        let mut out: Vec<String> = found
            .iter()
            .take(MAX_RESULTS)
            .map(|p| p.display().to_string())
            .collect();
        if total > MAX_RESULTS {
            out.push(format!("... and {} more", total - MAX_RESULTS));
        }
        Ok(ToolOutput::ok(out.join("\n")))
    }
}

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> String {
        "Search file contents with a regular expression, respecting .gitignore. \
         output_mode: `files_with_matches` (default), `content` (matching lines with line numbers) or `count`."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"},
                "glob": {"type": "string", "description": "Only search files matching this glob"},
                "output_mode": {"type": "string", "enum": ["files_with_matches", "content", "count"]},
                "case_insensitive": {"type": "boolean"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn title(&self, input: &Value) -> String {
        format!("Grep {}", input["pattern"].as_str().unwrap_or("?"))
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let re = match RegexBuilder::new(str_arg(&input, "pattern")?)
            .case_insensitive(input["case_insensitive"].as_bool().unwrap_or(false))
            .build()
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("invalid regex: {e}"))),
        };
        let glob = input["glob"].as_str().map(matcher).transpose()?;
        let mode = input["output_mode"]
            .as_str()
            .unwrap_or("files_with_matches")
            .to_string();
        let root = input["path"]
            .as_str()
            .map(|p| ctx.resolve(p))
            .unwrap_or_else(|| ctx.cwd.clone());

        let lines = tokio::task::spawn_blocking(move || {
            let files: Box<dyn Iterator<Item = PathBuf>> = if root.is_file() {
                Box::new(std::iter::once(root.clone()))
            } else {
                Box::new(walk(&root))
            };
            let mut out = Vec::new();
            for p in files {
                if let Some(g) = &glob
                    && !glob_match(g, &root, &p)
                {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else {
                    continue;
                };
                if bytes.iter().take(8000).any(|b| *b == 0) {
                    continue; // binary
                }
                let text = String::from_utf8_lossy(&bytes);
                match mode.as_str() {
                    "content" => {
                        for (i, l) in text.lines().enumerate() {
                            if re.is_match(l) {
                                out.push(format!("{}:{}:{}", p.display(), i + 1, l));
                            }
                        }
                    }
                    "count" => {
                        let n = text.lines().filter(|l| re.is_match(l)).count();
                        if n > 0 {
                            out.push(format!("{}:{n}", p.display()));
                        }
                    }
                    _ => {
                        if re.is_match(&text) {
                            out.push(p.display().to_string());
                        }
                    }
                }
                if out.len() > MAX_RESULTS {
                    out.truncate(MAX_RESULTS);
                    out.push("... [results truncated]".into());
                    break;
                }
            }
            out
        })
        .await?;
        if lines.is_empty() {
            Ok(ToolOutput::ok("No matches found"))
        } else {
            Ok(ToolOutput::ok(lines.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;

    #[tokio::test]
    async fn glob_and_grep() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello\n").unwrap();
        let c = ctx(dir.path());

        let out = Glob.call(json!({"pattern": "**/*.rs"}), &c).await.unwrap();
        assert!(out.content.contains("a.rs"));
        assert!(!out.content.contains("b.txt"));

        let out = Grep
            .call(json!({"pattern": "fn \\w+", "output_mode": "content"}), &c)
            .await
            .unwrap();
        assert!(out.content.contains("a.rs:1:fn main"));
    }
}
