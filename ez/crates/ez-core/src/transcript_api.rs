//! Machine-readable face of the transcript browser, shared by `ez sessions ... --json` (used
//! inside WSL by the desktop app) and the desktop app's local Tauri commands, so both produce the
//! same camelCase JSON.
//!
//! Transcripts are streamed line by line (never held in memory whole), tolerate truncated or
//! corrupt lines (counted in `skippedLines`) and are only ever read from
//! `<project>/.ez/sessions/`; session ids are validated so they cannot name another path.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use ez_llm::{ContentBlock, Message};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::transcript::{Record, Stamped, UserSource, sessions_dir};

/// Records returned by `read` when no limit is given.
pub const DEFAULT_READ_LIMIT: usize = 500;
/// The most records one `read` returns.
pub const MAX_READ_LIMIT: usize = 5000;
/// Search hits returned when no limit is given.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;
const FIRST_PROMPT_CHARS: usize = 300;

/// One row of the transcript list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionItem {
    pub id: String,
    pub created_at: String,
    /// Timestamp of the last readable record.
    pub updated_at: String,
    pub first_prompt: String,
    /// Mode at the end of the session.
    pub mode: String,
    /// Model at the end of the session (the pinned one, else the profile's).
    pub model: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Sum of the turns' durations.
    pub duration_ms: u64,
    pub size_bytes: u64,
    /// Readable records.
    pub record_count: usize,
    /// Lines that could not be parsed (a truncated tail, corruption).
    pub skipped_lines: usize,
}

/// A session's header: the list row plus what `session_start` recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMeta {
    #[serde(flatten)]
    pub summary: SessionItem,
    pub cwd: String,
    pub profile: String,
    pub ez_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub meta: TranscriptMeta,
    /// Normalized records `[offset, offset + limit)`, each with its `index` and `ts`.
    pub records: Vec<Value>,
    /// Readable records in the whole transcript.
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub skipped_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Session id.
    pub id: String,
    pub record_index: usize,
    /// The record's `type`.
    #[serde(rename = "type")]
    pub record_type: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOptions {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

// ---- ids and paths ----------------------------------------------------------------------------

/// Ids are file stems: letters, digits, `-` and `_` only (so `..`, separators and dots never occur).
pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("invalid session id `{}`", id.escape_debug());
    }
    Ok(())
}

/// The transcript file of `id`, verified to live directly inside the project's sessions directory
/// (symlinks pointing elsewhere are refused).
fn resolve(project: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    let dir = sessions_dir(project);
    let path = dir.join(format!("{id}.jsonl"));
    let canon = path
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("session not found: {id}"))?;
    let canon_dir = dir
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("session not found: {id}"))?;
    if canon.parent() != Some(canon_dir.as_path()) || !canon.is_file() {
        bail!("session not found: {id}");
    }
    Ok(canon)
}

/// Ids of the session files, newest (largest id) first.
fn session_ids(project: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(sessions_dir(project)) else {
        return vec![];
    };
    let mut ids: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "jsonl" {
                return None;
            }
            let stem = p.file_stem()?.to_str()?.to_string();
            validate_id(&stem).ok()?;
            Some(stem)
        })
        .collect();
    ids.sort_by(|a, b| b.cmp(a));
    ids
}

// ---- scanning ---------------------------------------------------------------------------------

/// Stream the stamped records of a file; `f(index, record)` sees each readable one. Returns the
/// number of lines skipped as unreadable.
fn for_each_record(path: &Path, mut f: impl FnMut(usize, Stamped)) -> Result<usize> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut index = 0;
    let mut skipped = 0;
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = match std::str::from_utf8(&buf) {
            Ok(l) => l.trim(),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Stamped>(line) {
            Ok(r) => {
                f(index, r);
                index += 1;
            }
            Err(_) => skipped += 1,
        }
    }
    Ok(skipped)
}

struct Summarizer {
    meta: Option<crate::transcript::SessionMeta>,
    updated_at: String,
    first_prompt: Option<String>,
    mode: String,
    pinned_model: Option<String>,
    turns: u32,
    tool_calls: u32,
    input_tokens: u64,
    output_tokens: u64,
    duration_ms: u64,
    records: usize,
}

impl Summarizer {
    fn new() -> Self {
        Self {
            meta: None,
            updated_at: String::new(),
            first_prompt: None,
            mode: String::new(),
            pinned_model: None,
            turns: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            duration_ms: 0,
            records: 0,
        }
    }

    fn feed(&mut self, r: &Stamped) {
        self.records += 1;
        self.updated_at.clone_from(&r.ts);
        match &r.record {
            Record::SessionStart(m) if self.meta.is_none() => {
                self.mode.clone_from(&m.mode);
                self.meta = Some(m.clone());
            }
            Record::UserMessage {
                message,
                source: UserSource::User,
            } if self.first_prompt.is_none() => {
                self.first_prompt = Some(message.text().chars().take(FIRST_PROMPT_CHARS).collect());
            }
            Record::AssistantMessage { usage, .. } => {
                self.input_tokens += usage.input_tokens;
                self.output_tokens += usage.output_tokens;
            }
            Record::ToolCall { .. } => self.tool_calls += 1,
            Record::ModeChange { to, .. } => self.mode.clone_from(to),
            Record::ModelChange { to, .. } => self.pinned_model.clone_from(to),
            Record::TurnEnd { duration_ms, .. } => {
                self.turns += 1;
                self.duration_ms += duration_ms;
            }
            _ => {}
        }
    }

    fn finish(self, id: &str, size_bytes: u64, skipped: usize) -> Result<TranscriptMeta> {
        let m = self
            .meta
            .with_context(|| format!("{id} does not start with session_start"))?;
        Ok(TranscriptMeta {
            summary: SessionItem {
                id: id.to_string(),
                created_at: m.created_at.clone(),
                updated_at: self.updated_at,
                first_prompt: self.first_prompt.unwrap_or_default(),
                mode: self.mode,
                model: self.pinned_model.unwrap_or_else(|| m.model.clone()),
                turns: self.turns,
                tool_calls: self.tool_calls,
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                duration_ms: self.duration_ms,
                size_bytes,
                record_count: self.records,
                skipped_lines: skipped,
            },
            cwd: m.cwd,
            profile: m.profile,
            ez_version: m.ez_version,
        })
    }
}

type CacheEntry = (SystemTime, u64, TranscriptMeta);

fn cache() -> &'static Mutex<HashMap<PathBuf, CacheEntry>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, CacheEntry>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn stamp(path: &Path) -> Result<(SystemTime, u64)> {
    let md = std::fs::metadata(path)?;
    Ok((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len()))
}

/// Summary of one transcript, cached by modification time and size.
fn summarize(path: &Path, id: &str) -> Result<TranscriptMeta> {
    let (mtime, size) = stamp(path)?;
    if let Some((m, s, meta)) = cache().lock().unwrap().get(path)
        && *m == mtime
        && *s == size
    {
        return Ok(meta.clone());
    }
    let mut sum = Summarizer::new();
    let skipped = for_each_record(path, |_, r| sum.feed(&r))?;
    let meta = sum.finish(id, size, skipped)?;
    cache()
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), (mtime, size, meta.clone()));
    Ok(meta)
}

// ---- public API -------------------------------------------------------------------------------

fn contains_ci(hay: &str, needle: &str) -> bool {
    fold(hay).contains(&fold(needle))
}

/// Sessions of the project, most recently updated first. `query` keeps those whose id, first
/// prompt, mode or model contains it (case-insensitive).
pub fn list(project: &Path, query: Option<&str>) -> Result<Vec<SessionItem>> {
    let mut out = Vec::new();
    for id in session_ids(project) {
        let Ok(path) = resolve(project, &id) else {
            continue;
        };
        // Files that are not transcripts (no session_start) are left out of the list.
        let Ok(meta) = summarize(&path, &id) else {
            continue;
        };
        let s = meta.summary;
        if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty())
            && ![&s.id, &s.first_prompt, &s.mode, &s.model]
                .iter()
                .any(|h| contains_ci(h, q))
        {
            continue;
        }
        out.push(s);
    }
    out.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// A page of a transcript's records plus its header. One streaming pass computes both.
pub fn read(project: &Path, id: &str, opts: ReadOptions) -> Result<ReadResult> {
    let path = resolve(project, id)?;
    let offset = opts.offset.unwrap_or(0);
    let limit = opts
        .limit
        .unwrap_or(DEFAULT_READ_LIMIT)
        .clamp(1, MAX_READ_LIMIT);
    let (mtime, size) = stamp(&path)?;
    let mut sum = Summarizer::new();
    let mut records = Vec::new();
    let skipped = for_each_record(&path, |i, r| {
        sum.feed(&r);
        if i >= offset && i < offset.saturating_add(limit) {
            records.push(normalize(i, &r));
        }
    })?;
    let meta = sum.finish(id, size, skipped)?;
    cache()
        .lock()
        .unwrap()
        .insert(path, (mtime, size, meta.clone()));
    Ok(ReadResult {
        total: meta.summary.record_count,
        skipped_lines: skipped,
        meta,
        records,
        offset,
        limit,
    })
}

/// Case-insensitive search over prompts, assistant text, reasoning, tool names/inputs/outputs,
/// diffs and hook output. One hit per matching record, sessions newest first.
pub fn search(project: &Path, text: &str, limit: Option<usize>) -> Result<Vec<SearchHit>> {
    let needle = fold(text.trim());
    let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, 1000);
    let mut hits = Vec::new();
    if needle.is_empty() {
        return Ok(hits);
    }
    for id in session_ids(project) {
        if hits.len() >= limit {
            break;
        }
        let Ok(path) = resolve(project, &id) else {
            continue;
        };
        let _ = for_each_record(&path, |i, r| {
            if hits.len() >= limit {
                return;
            }
            for t in searchable(&r.record) {
                if let Some(snippet) = snippet_of(&t, &needle) {
                    hits.push(SearchHit {
                        id: id.clone(),
                        record_index: i,
                        record_type: type_name(&r.record).to_string(),
                        snippet,
                    });
                    break;
                }
            }
        });
    }
    Ok(hits)
}

// ---- searching helpers ------------------------------------------------------------------------

/// Lowercase one char to one char, so char positions of the folded text equal the original's.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// A one-line excerpt around the first occurrence of the folded `needle`.
fn snippet_of(text: &str, needle: &str) -> Option<String> {
    let folded: Vec<char> = fold(text).chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || folded.len() < n.len() {
        return None;
    }
    let pos = folded.windows(n.len()).position(|w| w == n.as_slice())?;
    let chars: Vec<char> = text.chars().collect();
    let start = pos.saturating_sub(60);
    let end = (pos + n.len() + 100).min(chars.len());
    let body: String = chars[start..end].iter().collect();
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        body,
        if end < chars.len() { "…" } else { "" }
    ))
}

fn block_texts(m: &Message, out: &mut Vec<String>) {
    for b in &m.content {
        match b {
            ContentBlock::Text { text } => out.push(text.clone()),
            ContentBlock::Thinking { thinking, .. } => out.push(thinking.clone()),
            ContentBlock::ToolUse { name, input, .. } => {
                out.push(name.clone());
                out.push(input.to_string());
            }
            ContentBlock::ToolResult { content, .. } => out.push(content.clone()),
            ContentBlock::RedactedThinking { .. } => {}
        }
    }
}

fn searchable(r: &Record) -> Vec<String> {
    let mut v = Vec::new();
    match r {
        Record::UserMessage { message, .. } | Record::AssistantMessage { message, .. } => {
            block_texts(message, &mut v)
        }
        Record::ToolCall {
            name,
            input,
            title,
            kind,
            ..
        } => v.extend([name.clone(), title.clone(), kind.clone(), input.to_string()]),
        Record::ToolResult {
            name,
            output,
            edits,
            ..
        } => {
            v.push(name.clone());
            v.push(output.clone());
            for e in edits {
                v.push(e.path.clone());
                v.push(e.diff.clone());
            }
        }
        Record::HookRun {
            event,
            command,
            output,
            ..
        } => v.extend([event.clone(), command.clone(), output.clone()]),
        Record::Compaction { summary, .. } => v.push(summary.clone()),
        Record::ModeChange {
            from, to, reason, ..
        } => v.extend([from.clone(), to.clone(), reason.clone()]),
        Record::McpServers { servers } => {
            for s in servers {
                v.push(s.name.clone());
                v.extend(s.error.clone());
            }
        }
        _ => {}
    }
    v
}

// ---- normalized records -----------------------------------------------------------------------

pub fn type_name(r: &Record) -> &'static str {
    match r {
        Record::SessionStart(_) => "session_start",
        Record::UserMessage { .. } => "user_message",
        Record::AssistantMessage { .. } => "assistant_message",
        Record::ToolCall { .. } => "tool_call",
        Record::ToolResult { .. } => "tool_result",
        Record::HookRun { .. } => "hook_run",
        Record::ModeChange { .. } => "mode_change",
        Record::ModelChange { .. } => "model_change",
        Record::Compaction { .. } => "compaction",
        Record::TurnEnd { .. } => "turn_end",
        Record::SessionResumed {} => "session_resumed",
        Record::McpServers { .. } => "mcp_servers",
    }
}

fn enum_str<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn blocks(m: &Message) -> Value {
    Value::Array(
        m.content
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => json!({"type":"text","text":text}),
                ContentBlock::Thinking { thinking, .. } => {
                    json!({"type":"thinking","thinking":thinking})
                }
                ContentBlock::RedactedThinking { .. } => json!({"type":"redacted_thinking"}),
                ContentBlock::ToolUse { id, name, input } => {
                    json!({"type":"tool_use","id":id,"name":name,"input":input})
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => json!({"type":"tool_result","toolUseId":tool_use_id,"content":content,"isError":is_error}),
            })
            .collect(),
    )
}

/// One record as camelCase JSON: `{index, ts, type, ...}`. Message content becomes a `content`
/// array of `text` / `thinking` / `redacted_thinking` / `tool_use` / `tool_result` blocks (the
/// signature of thinking blocks is dropped).
pub fn normalize(index: usize, s: &Stamped) -> Value {
    let mut v = match &s.record {
        Record::SessionStart(m) => json!({
            "id": m.id, "cwd": m.cwd, "createdAt": m.created_at,
            "ezVersion": m.ez_version, "profile": m.profile,
            "model": m.model, "mode": m.mode,
        }),
        Record::UserMessage { message, source } => {
            json!({"source": enum_str(source), "content": blocks(message)})
        }
        Record::AssistantMessage {
            message,
            request_id,
            model,
            stop_reason,
            usage,
            metrics,
        } => json!({
            "requestId": request_id, "model": model, "stopReason": enum_str(stop_reason),
            "usage": {"inputTokens": usage.input_tokens, "outputTokens": usage.output_tokens},
            "metrics": {
                "durationMs": metrics.duration_ms,
                "timeToFirstTokenMs": metrics.time_to_first_token_ms,
                "outputTokensPerSec": metrics.output_tokens_per_sec,
            },
            "content": blocks(message),
        }),
        Record::ToolCall {
            tool_use_id,
            name,
            input,
            title,
            kind,
        } => {
            json!({"toolUseId": tool_use_id, "name": name, "input": input, "title": title, "kind": kind})
        }
        Record::ToolResult {
            tool_use_id,
            name,
            output,
            is_error,
            permission,
            duration_ms,
            edits,
        } => json!({
            "toolUseId": tool_use_id, "name": name, "output": output, "isError": is_error,
            "permission": enum_str(permission), "durationMs": duration_ms,
            "edits": edits.iter().map(|e| json!({"path": e.path, "diff": e.diff})).collect::<Vec<_>>(),
        }),
        Record::HookRun {
            event,
            command,
            success,
            exit_code,
            output,
            duration_ms,
        } => json!({
            "event": event, "command": command, "success": success, "exitCode": exit_code,
            "output": output, "durationMs": duration_ms,
        }),
        Record::ModeChange {
            from,
            to,
            reason,
            initiated_by,
        } => {
            json!({"from": from, "to": to, "reason": reason, "initiatedBy": enum_str(initiated_by)})
        }
        Record::ModelChange { from, to } => json!({"from": from, "to": to}),
        Record::Compaction {
            summary,
            replaced_messages,
        } => json!({"summary": summary, "replacedMessages": replaced_messages}),
        Record::TurnEnd {
            stop_reason,
            duration_ms,
            model_requests,
            tool_calls,
            input_tokens,
            output_tokens,
        } => json!({
            "stopReason": stop_reason, "durationMs": duration_ms, "modelRequests": model_requests,
            "toolCalls": tool_calls, "inputTokens": input_tokens, "outputTokens": output_tokens,
        }),
        Record::SessionResumed {} => json!({}),
        Record::McpServers { servers } => json!({
            "servers": servers.iter().map(|s| json!({
                "name": s.name, "ok": s.ok, "tools": s.tools, "error": s.error,
            })).collect::<Vec<_>>(),
        }),
    };
    let obj = v.as_object_mut().expect("record JSON is an object");
    obj.insert("index".into(), json!(index));
    obj.insert("ts".into(), json!(s.ts));
    obj.insert("type".into(), json!(type_name(&s.record)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{
        EditRecord, Initiator, McpServerRecord, Metrics, PermissionOutcome, SessionMeta,
        TranscriptWriter,
    };
    use ez_llm::{Role, StopReason, Usage};
    use std::io::Write;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.into(),
            cwd: "/x".into(),
            created_at: "2026-03-01T10:00:00.000Z".into(),
            ez_version: "0.1.0".into(),
            profile: "p".into(),
            model: "m1".into(),
            mode: "plan".into(),
        }
    }

    fn assistant(text: &str, i: u64, o: u64) -> Record {
        Record::AssistantMessage {
            message: Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "pondering the Flux Capacitor".into(),
                        signature: Some("sig".into()),
                    },
                    ContentBlock::Text { text: text.into() },
                ],
            },
            request_id: 1,
            model: "m1".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: i,
                output_tokens: o,
            },
            metrics: Metrics {
                duration_ms: 900,
                time_to_first_token_ms: Some(120),
                output_tokens_per_sec: Some(42.5),
            },
        }
    }

    /// A transcript with every record type.
    fn full(project: &Path, id: &str) -> PathBuf {
        let mut w = TranscriptWriter::create(project, meta(id)).unwrap();
        let recs = vec![
            Record::McpServers {
                servers: vec![McpServerRecord {
                    name: "fs".into(),
                    ok: false,
                    tools: 0,
                    error: Some("spawn failed".into()),
                }],
            },
            Record::UserMessage {
                message: Message::user_text("Fix the LOGIN bug"),
                source: UserSource::User,
            },
            assistant("Looking at auth.rs", 100, 20),
            Record::ToolCall {
                tool_use_id: "t1".into(),
                name: "Edit".into(),
                input: json!({"file_path":"src/auth.rs"}),
                title: "Edit src/auth.rs".into(),
                kind: "edit".into(),
            },
            Record::ToolResult {
                tool_use_id: "t1".into(),
                name: "Edit".into(),
                output: "ok, edited".into(),
                is_error: false,
                permission: PermissionOutcome::AskApproved,
                duration_ms: 12,
                edits: vec![EditRecord {
                    path: "src/auth.rs".into(),
                    diff: "@@ -1 +1 @@\n-old\n+new\n".into(),
                }],
            },
            Record::HookRun {
                event: "post_tool".into(),
                command: "cargo fmt".into(),
                success: false,
                exit_code: Some(2),
                output: "fmt complained".into(),
                duration_ms: 30,
            },
            Record::ModeChange {
                from: "plan".into(),
                to: "build".into(),
                reason: "plan approved".into(),
                initiated_by: Initiator::Agent,
            },
            Record::ModelChange {
                from: None,
                to: Some("m2".into()),
            },
            Record::UserMessage {
                message: Message::user_text("tool output"),
                source: UserSource::ToolResults,
            },
            assistant("Done", 50, 5),
            Record::Compaction {
                summary: "Summary of work".into(),
                replaced_messages: 4,
            },
            Record::TurnEnd {
                stop_reason: "end_turn".into(),
                duration_ms: 2500,
                model_requests: 2,
                tool_calls: 1,
                input_tokens: 150,
                output_tokens: 25,
            },
            Record::SessionResumed {},
        ];
        for r in &recs {
            w.append(r).unwrap();
        }
        w.path().to_path_buf()
    }

    #[test]
    fn summary_fields() {
        let d = tempfile::tempdir().unwrap();
        let p = full(d.path(), "20260301-100000-aaaaaa");
        let l = list(d.path(), None).unwrap();
        assert_eq!(l.len(), 1);
        let s = &l[0];
        assert_eq!(s.id, "20260301-100000-aaaaaa");
        assert_eq!(s.first_prompt, "Fix the LOGIN bug");
        assert_eq!(s.mode, "build");
        assert_eq!(s.model, "m2");
        assert_eq!((s.turns, s.tool_calls), (1, 1));
        assert_eq!((s.input_tokens, s.output_tokens), (150, 25));
        assert_eq!(s.duration_ms, 2500);
        assert_eq!(s.size_bytes, std::fs::metadata(p).unwrap().len());
        assert_eq!(s.record_count, 14);
        assert_eq!(s.skipped_lines, 0);
        let v = serde_json::to_value(s).unwrap();
        for k in [
            "createdAt",
            "updatedAt",
            "firstPrompt",
            "toolCalls",
            "inputTokens",
            "outputTokens",
            "durationMs",
            "sizeBytes",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
    }

    #[test]
    fn list_filters_sorts_and_caches() {
        let d = tempfile::tempdir().unwrap();
        full(d.path(), "20260301-100000-aaaaaa");
        let mut w = TranscriptWriter::create(d.path(), meta("20260302-100000-bbbbbb")).unwrap();
        w.append(&Record::UserMessage {
            message: Message::user_text("other"),
            source: UserSource::User,
        })
        .unwrap();
        let l = list(d.path(), None).unwrap();
        assert_eq!(l[0].id, "20260302-100000-bbbbbb"); // updated last
        assert_eq!(list(d.path(), Some("login")).unwrap().len(), 1);
        assert_eq!(list(d.path(), Some("M2")).unwrap().len(), 1);
        assert!(list(d.path(), Some("nomatch")).unwrap().is_empty());
        // Appending changes the size, so the cache entry is refreshed.
        w.append(&Record::TurnEnd {
            stop_reason: "end_turn".into(),
            duration_ms: 5,
            model_requests: 1,
            tool_calls: 0,
            input_tokens: 1,
            output_tokens: 1,
        })
        .unwrap();
        let l = list(d.path(), Some("other")).unwrap();
        assert_eq!(l[0].turns, 1);
        // Not a transcript, and not a session file at all: ignored.
        std::fs::write(sessions_dir(d.path()).join("junk.jsonl"), "{}\n").unwrap();
        std::fs::write(sessions_dir(d.path()).join("notes.txt"), "x").unwrap();
        assert_eq!(list(d.path(), None).unwrap().len(), 2);
        assert!(
            list(tempfile::tempdir().unwrap().path(), None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn read_pages_and_normalizes() {
        let d = tempfile::tempdir().unwrap();
        full(d.path(), "s1");
        let r = read(d.path(), "s1", ReadOptions::default()).unwrap();
        assert_eq!(r.total, 14);
        assert_eq!(r.records.len(), 14);
        assert_eq!(r.meta.cwd, "/x");
        assert_eq!(r.meta.summary.model, "m2");
        let types: Vec<&str> = r
            .records
            .iter()
            .map(|v| v["type"].as_str().unwrap())
            .collect();
        for t in [
            "session_start",
            "user_message",
            "assistant_message",
            "tool_call",
            "tool_result",
            "hook_run",
            "mode_change",
            "model_change",
            "compaction",
            "turn_end",
            "session_resumed",
            "mcp_servers",
        ] {
            assert!(types.contains(&t), "missing {t}");
        }
        let a = &r.records[3];
        assert_eq!(a["requestId"], 1);
        assert_eq!(a["stopReason"], "end_turn");
        assert_eq!(a["usage"]["inputTokens"], 100);
        assert_eq!(a["metrics"]["timeToFirstTokenMs"], 120);
        assert_eq!(a["content"][0]["type"], "thinking");
        assert!(a["content"][0].get("signature").is_none());
        let tr = &r.records[5];
        assert_eq!(tr["permission"], "ask_approved");
        assert_eq!(tr["edits"][0]["path"], "src/auth.rs");
        assert_eq!(r.records[7]["initiatedBy"], "agent");
        assert_eq!(r.records[1]["servers"][0]["error"], "spawn failed");

        let p = read(
            d.path(),
            "s1",
            ReadOptions {
                offset: Some(4),
                limit: Some(3),
            },
        )
        .unwrap();
        assert_eq!(p.total, 14);
        assert_eq!(p.records.len(), 3);
        assert_eq!(p.records[0]["index"], 4);
        assert_eq!(p.records[2]["index"], 6);
        let past = read(
            d.path(),
            "s1",
            ReadOptions {
                offset: Some(99),
                limit: None,
            },
        )
        .unwrap();
        assert!(past.records.is_empty());
        assert_eq!(past.total, 14);
        let huge = read(
            d.path(),
            "s1",
            ReadOptions {
                offset: None,
                limit: Some(usize::MAX),
            },
        )
        .unwrap();
        assert_eq!(huge.limit, MAX_READ_LIMIT);
    }

    #[test]
    fn corrupt_and_truncated_lines_are_counted() {
        let d = tempfile::tempdir().unwrap();
        let p = full(d.path(), "s1");
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"this is not json\n\n").unwrap();
        f.write_all(&[0xff, 0xfe, b'\n']).unwrap();
        f.write_all(b"{\"ts\":\"x\",\"type\":\"user_mess").unwrap();
        let r = read(d.path(), "s1", ReadOptions::default()).unwrap();
        assert_eq!(r.total, 14);
        assert_eq!(r.skipped_lines, 3);
        assert_eq!(r.meta.summary.skipped_lines, 3);
        assert_eq!(list(d.path(), None).unwrap()[0].skipped_lines, 3);
        // A corrupt line in the middle keeps the later indexes consecutive.
        let d2 = tempfile::tempdir().unwrap();
        let p2 = full(d2.path(), "s2");
        let text = std::fs::read_to_string(&p2).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines.insert(3, "garbage");
        std::fs::write(&p2, lines.join("\n") + "\n").unwrap();
        let r = read(d2.path(), "s2", ReadOptions::default()).unwrap();
        assert_eq!((r.total, r.skipped_lines), (14, 1));
        assert_eq!(r.records[13]["index"], 13);
    }

    #[test]
    fn missing_session_start_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(sessions_dir(d.path())).unwrap();
        std::fs::write(
            sessions_dir(d.path()).join("bad.jsonl"),
            "{\"ts\":\"t\",\"type\":\"session_resumed\"}\n",
        )
        .unwrap();
        let e = read(d.path(), "bad", ReadOptions::default()).unwrap_err();
        assert!(format!("{e:#}").contains("session_start"), "{e:#}");
        assert!(
            read(d.path(), "nope", ReadOptions::default())
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
    }

    #[test]
    fn search_covers_every_text_source() {
        let d = tempfile::tempdir().unwrap();
        full(d.path(), "s1");
        let find = |t: &str| search(d.path(), t, None).unwrap();
        let h = find("login");
        assert_eq!(h.len(), 1);
        assert_eq!(
            (
                h[0].id.as_str(),
                h[0].record_index,
                h[0].record_type.as_str()
            ),
            ("s1", 2, "user_message")
        );
        assert!(h[0].snippet.contains("LOGIN"));
        assert_eq!(find("flux capacitor")[0].record_type, "assistant_message");
        assert_eq!(find("src/auth.rs")[0].record_type, "tool_call");
        assert!(
            find("ok, edited")
                .iter()
                .any(|h| h.record_type == "tool_result")
        );
        assert!(find("+new").iter().any(|h| h.record_type == "tool_result"));
        assert!(
            find("fmt complained")
                .iter()
                .any(|h| h.record_type == "hook_run")
        );
        assert!(
            find("plan approved")
                .iter()
                .any(|h| h.record_type == "mode_change")
        );
        assert!(
            find("summary of work")
                .iter()
                .any(|h| h.record_type == "compaction")
        );
        assert!(
            find("spawn failed")
                .iter()
                .any(|h| h.record_type == "mcp_servers")
        );
        assert!(find("zzzz-not-there").is_empty());
        assert!(find("   ").is_empty());
        assert_eq!(search(d.path(), "e", Some(2)).unwrap().len(), 2);
    }

    #[test]
    fn snippet_is_bounded_and_unicode_safe() {
        let text = format!("{} İstanbul NEEDLE {}", "x".repeat(200), "y".repeat(300));
        let s = snippet_of(&text, &fold("needle")).unwrap();
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("NEEDLE"));
        assert!(s.chars().count() < 200);
        assert!(snippet_of("héllo wörld", &fold("WÖRLD")).is_some());
    }

    #[test]
    fn ids_cannot_escape_the_sessions_directory() {
        let d = tempfile::tempdir().unwrap();
        full(d.path(), "s1");
        // A file next to (and above) the sessions directory that must stay unreachable.
        std::fs::write(
            d.path().join(".ez/secret.jsonl"),
            "{\"ts\":\"t\",\"type\":\"session_resumed\"}\n",
        )
        .unwrap();
        std::fs::write(d.path().join("outside.jsonl"), "x\n").unwrap();
        for bad in [
            "",
            "..",
            "../secret",
            "../../outside",
            "a/b",
            "a\\b",
            "s1.jsonl",
            "s1\0",
            "/etc/passwd",
            ".hidden",
            "s 1",
            "é",
        ] {
            assert!(validate_id(bad).is_err(), "{bad:?}");
            assert!(
                read(d.path(), bad, ReadOptions::default()).is_err(),
                "{bad:?}"
            );
        }
        assert!(validate_id(&"a".repeat(129)).is_err());
        assert!(validate_id("20260301-100000-aaaaaa").is_ok());
        #[cfg(unix)]
        {
            // A symlink inside the directory that points outside is refused and not listed.
            std::os::unix::fs::symlink(
                d.path().join(".ez/secret.jsonl"),
                sessions_dir(d.path()).join("link.jsonl"),
            )
            .unwrap();
            let e = read(d.path(), "link", ReadOptions::default()).unwrap_err();
            assert!(e.to_string().contains("not found"), "{e}");
            assert_eq!(list(d.path(), None).unwrap().len(), 1);
            assert!(
                search(d.path(), "session_resumed", None)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
