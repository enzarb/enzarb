use crate::{FileEdit, Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use async_trait::async_trait;
use serde_json::{Value, json};

const MAX_LINE: usize = 2000;
const DEFAULT_LIMIT: usize = 2000;

pub struct Read;

#[async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> String {
        "Read a text file. Returns lines prefixed with line numbers (cat -n style). \
         Use offset/limit (1-based line numbers) for large files."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Path to the file (absolute or relative to the project root)"},
                "offset": {"type": "integer", "description": "Line number to start reading from (1-based)"},
                "limit": {"type": "integer", "description": "Number of lines to read"}
            },
            "required": ["file_path"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn title(&self, input: &Value) -> String {
        format!("Read {}", input["file_path"].as_str().unwrap_or("?"))
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let path = ctx.resolve(str_arg(&input, "file_path")?);
        let text = match tokio::fs::read(&path).await {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => return Ok(ToolOutput::error(format!("{}: {e}", path.display()))),
        };
        if text.is_empty() {
            return Ok(ToolOutput::ok("(file is empty)"));
        }
        let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = input["limit"]
            .as_u64()
            .map(|l| l as usize)
            .unwrap_or(DEFAULT_LIMIT);
        let total = text.lines().count();
        let mut out = String::new();
        for (i, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
            let line = if line.len() > MAX_LINE {
                let mut end = MAX_LINE;
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}… [line truncated]", &line[..end])
            } else {
                line.to_string()
            };
            out.push_str(&format!("{:>6}\t{}\n", i + 1, line));
        }
        let shown_end = (offset - 1 + limit).min(total);
        if shown_end < total {
            out.push_str(&format!(
                "\n[showing lines {offset}-{shown_end} of {total}; use offset to read more]\n"
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct Write;

#[async_trait]
impl Tool for Write {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> String {
        "Create or overwrite a file with the given content. Prefer Edit for changing existing files."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["file_path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    fn title(&self, input: &Value) -> String {
        format!("Write {}", input["file_path"].as_str().unwrap_or("?"))
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let path = ctx.resolve(str_arg(&input, "file_path")?);
        let content = str_arg(&input, "content")?.to_string();
        let old = tokio::fs::read_to_string(&path).await.ok();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Err(e) = tokio::fs::write(&path, &content).await {
            return Ok(ToolOutput::error(format!("{}: {e}", path.display())));
        }
        let verb = if old.is_some() { "Updated" } else { "Created" };
        Ok(ToolOutput {
            content: format!(
                "{verb} {} ({} lines)",
                path.display(),
                content.lines().count()
            ),
            edits: vec![FileEdit {
                path,
                old_text: old,
                new_text: content,
            }],
            ..Default::default()
        })
    }
}

pub struct Edit;

#[async_trait]
impl Tool for Edit {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> String {
        "Replace an exact string in a file. `old_string` must match exactly (including whitespace) \
         and be unique unless `replace_all` is true."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean", "default": false}
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    fn title(&self, input: &Value) -> String {
        format!("Edit {}", input["file_path"].as_str().unwrap_or("?"))
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let path = ctx.resolve(str_arg(&input, "file_path")?);
        let old_string = str_arg(&input, "old_string")?;
        let new_string = str_arg(&input, "new_string")?;
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);
        if old_string == new_string {
            return Ok(ToolOutput::error("old_string and new_string are identical"));
        }
        let old = match tokio::fs::read_to_string(&path).await {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::error(format!("{}: {e}", path.display()))),
        };
        let count = old.matches(old_string).count();
        let new = match (count, replace_all) {
            (0, _) => {
                return Ok(ToolOutput::error(format!(
                    "old_string not found in {}",
                    path.display()
                )));
            }
            (1, _) | (_, true) => old.replace(old_string, new_string),
            (n, false) => {
                return Ok(ToolOutput::error(format!(
                    "old_string occurs {n} times in {}; add context to make it unique or set replace_all",
                    path.display()
                )));
            }
        };
        tokio::fs::write(&path, &new).await?;
        Ok(ToolOutput {
            content: format!("Edited {} ({count} replacement(s))", path.display()),
            edits: vec![FileEdit {
                path,
                old_text: Some(old),
                new_text: new,
            }],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;

    #[tokio::test]
    async fn write_read_edit_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let out = Write
            .call(
                json!({"file_path": "a/b.txt", "content": "one\ntwo\ntwo\n"}),
                &c,
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(out.edits.len(), 1);

        let out = Read
            .call(json!({"file_path": "a/b.txt"}), &c)
            .await
            .unwrap();
        assert!(out.content.contains("     2\ttwo"));

        let out = Edit
            .call(
                json!({"file_path": "a/b.txt", "old_string": "two", "new_string": "2"}),
                &c,
            )
            .await
            .unwrap();
        assert!(out.is_error, "ambiguous edit should fail");

        let out = Edit
            .call(
                json!({"file_path": "a/b.txt", "old_string": "two", "new_string": "2", "replace_all": true}),
                &c,
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap(),
            "one\n2\n2\n"
        );
    }
}
