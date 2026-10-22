//! Pure TUI state: no terminal and no I/O, so every transition is unit-testable.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ez_acp::schema::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, PlanEntry,
    RequestPermissionOutcome, RequestPermissionRequest, SelectedPermissionOutcome, SessionMode,
    SessionUpdate, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::client::{Answer, AskRequest, AskResponse, Question, SessionEntry};

/// What the event loop must do after a key press.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Send this prompt (the user block is already in the transcript).
    Submit(String),
    Cancel,
    SetMode(String),
    NewSession,
    ListSessions,
    LoadSession(String),
    /// Pin (or with `None` clear) the session model.
    SetModel(Option<String>),
    /// Run the setup wizard.
    Setup,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffKind {
    Add,
    Del,
    Hunk,
}

#[derive(Debug, Clone)]
pub struct ToolView {
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    pub text: Vec<String>,
    pub diff: Vec<(DiffKind, String)>,
}

#[derive(Debug, Clone)]
pub enum Block {
    User(String),
    Agent(String),
    Thought(String),
    Tool(ToolView),
    Notice(String),
    Error(String),
}

pub struct PermissionModal {
    pub title: String,
    pub summary: Vec<String>,
    pub options: Vec<PermissionOption>,
    pub selected: usize,
    reply: Option<oneshot::Sender<RequestPermissionOutcome>>,
}

#[derive(Debug, Clone, Default)]
pub struct QState {
    pub cursor: usize,
    /// One flag per option, plus a trailing one for "Other…".
    pub checked: Vec<bool>,
    pub other_text: String,
    pub editing: bool,
}

pub struct AskModal {
    pub questions: Vec<Question>,
    pub page: usize,
    pub states: Vec<QState>,
    reply: Option<oneshot::Sender<AskResponse>>,
}

pub struct PickerModal {
    pub sessions: Vec<SessionEntry>,
    pub selected: usize,
}

/// `/model` picker: index 0 is "profile/mode default".
pub struct ModelPickerModal {
    pub models: Vec<String>,
    pub selected: usize,
}

pub enum Modal {
    Permission(PermissionModal),
    Ask(AskModal),
    Picker(PickerModal),
    Models(ModelPickerModal),
}

pub struct App {
    pub blocks: Vec<Block>,
    tool_index: HashMap<String, usize>,
    pub plan: Vec<PlanEntry>,
    pub session_id: Option<String>,
    pub mode_id: String,
    pub modes: Vec<SessionMode>,
    pub model_label: String,
    /// Models listed for the session profile (`/model`).
    pub models: Vec<String>,
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    hist_pos: Option<usize>,
    pub running: bool,
    pub tick: u64,
    /// Lines scrolled up from the bottom; 0 follows the output.
    pub scroll: usize,
    pub max_scroll: Cell<usize>,
    pub page_height: Cell<usize>,
    pub show_thoughts: bool,
    pub modal: Option<Modal>,
    modal_queue: VecDeque<Modal>,
    pub quit_armed: bool,
    pub should_quit: bool,
    pub queue: VecDeque<String>,
    pub chars_streamed: usize,
    /// Real context usage from the agent's `usage_update`: (tokens used, context window).
    pub usage: Option<(u64, u64)>,
}

pub const HELP: &str = "/help  this help\n/mode <id>  switch mode (shift+tab cycles)\n/modes  list modes\n/model [id|default]  pick the model for this session\n/setup  run the setup wizard\n/clear  start a new session\n/resume, /sessions  pick a previous session\n/init  how to configure modes\n/quit  exit\nenter: send  alt+enter / shift+enter / ctrl+j: newline  esc: interrupt  ctrl+t: thoughts  pgup/pgdn: scroll  ctrl+c twice: quit";

impl App {
    pub fn new(model_label: impl Into<String>) -> Self {
        Self {
            blocks: vec![],
            tool_index: HashMap::new(),
            plan: vec![],
            session_id: None,
            mode_id: String::new(),
            modes: vec![],
            model_label: model_label.into(),
            models: vec![],
            input: String::new(),
            cursor: 0,
            history: vec![],
            hist_pos: None,
            running: false,
            tick: 0,
            scroll: 0,
            max_scroll: Cell::new(0),
            page_height: Cell::new(10),
            show_thoughts: false,
            modal: None,
            modal_queue: VecDeque::new(),
            quit_armed: false,
            should_quit: false,
            queue: VecDeque::new(),
            chars_streamed: 0,
            usage: None,
        }
    }

    /// Forget the transcript (new or loaded session).
    pub fn reset_for_session(&mut self, session_id: Option<String>) {
        self.blocks.clear();
        self.tool_index.clear();
        self.plan.clear();
        self.session_id = session_id;
        self.scroll = 0;
        self.running = false;
        self.queue.clear();
        self.chars_streamed = 0;
        self.usage = None;
    }

    pub fn set_modes(&mut self, modes: Vec<SessionMode>, current: String) {
        self.modes = modes;
        if !current.is_empty() {
            self.mode_id = current;
        }
    }

    pub fn notice(&mut self, s: impl Into<String>) {
        self.blocks.push(Block::Notice(s.into()));
        self.scroll = 0;
    }

    pub fn error(&mut self, s: impl Into<String>) {
        self.blocks.push(Block::Error(s.into()));
        self.scroll = 0;
    }

    // ---- session updates -------------------------------------------------

    fn text_of(c: &ContentBlock) -> Option<&str> {
        match c {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    pub fn handle_update(&mut self, u: SessionUpdate) {
        match u {
            SessionUpdate::UserMessageChunk(ContentChunk { content, .. }) => {
                if let Some(t) = Self::text_of(&content) {
                    match self.blocks.last_mut() {
                        Some(Block::User(s)) => s.push_str(t),
                        _ => self.blocks.push(Block::User(t.to_string())),
                    }
                }
            }
            SessionUpdate::AgentMessageChunk(ContentChunk { content, .. }) => {
                if let Some(t) = Self::text_of(&content) {
                    self.chars_streamed += t.len();
                    match self.blocks.last_mut() {
                        Some(Block::Agent(s)) => s.push_str(t),
                        _ => self.blocks.push(Block::Agent(t.to_string())),
                    }
                }
            }
            SessionUpdate::AgentThoughtChunk(ContentChunk { content, .. }) => {
                if let Some(t) = Self::text_of(&content) {
                    self.chars_streamed += t.len();
                    match self.blocks.last_mut() {
                        Some(Block::Thought(s)) => s.push_str(t),
                        _ => self.blocks.push(Block::Thought(t.to_string())),
                    }
                }
            }
            SessionUpdate::ToolCall(tc) => self.on_tool_call(tc),
            SessionUpdate::ToolCallUpdate(up) => self.on_tool_update(up),
            SessionUpdate::Plan(p) => self.plan = p.entries,
            SessionUpdate::CurrentModeUpdate(m) => self.mode_id = m.current_mode_id.to_string(),
            SessionUpdate::Notice(n) => self.notice(n.title),
            SessionUpdate::UsageUpdate(u) => self.usage = Some((u.used, u.size)),
            // Commands, config options, session info, compaction: not shown.
            _ => {}
        }
    }

    fn on_tool_call(&mut self, tc: ToolCall) {
        let mut view = ToolView {
            title: tc.title.clone(),
            kind: tc.kind,
            status: tc.status,
            text: vec![],
            diff: vec![],
        };
        apply_content(&mut view, &tc.content);
        match self.tool_index.get(&*tc.tool_call_id.0) {
            Some(&i) => self.blocks[i] = Block::Tool(view),
            None => {
                self.tool_index
                    .insert(tc.tool_call_id.to_string(), self.blocks.len());
                self.blocks.push(Block::Tool(view));
            }
        }
    }

    fn on_tool_update(&mut self, up: ToolCallUpdate) {
        let idx = match self.tool_index.get(&*up.tool_call_id.0) {
            Some(&i) => i,
            None => {
                self.tool_index
                    .insert(up.tool_call_id.to_string(), self.blocks.len());
                self.blocks.push(Block::Tool(ToolView {
                    title: up
                        .fields
                        .title
                        .clone()
                        .unwrap_or_else(|| up.tool_call_id.to_string()),
                    kind: ToolKind::Other,
                    status: ToolCallStatus::Pending,
                    text: vec![],
                    diff: vec![],
                }));
                self.blocks.len() - 1
            }
        };
        if let Block::Tool(v) = &mut self.blocks[idx] {
            if let Some(t) = up.fields.title {
                v.title = t;
            }
            if let Some(k) = up.fields.kind {
                v.kind = k;
            }
            if let Some(s) = up.fields.status {
                v.status = s;
            }
            if let Some(c) = &up.fields.content {
                v.text.clear();
                v.diff.clear();
                apply_content(v, c);
            }
        }
    }

    // ---- requests ---------------------------------------------------------

    pub fn push_modal(&mut self, m: Modal) {
        if self.modal.is_none() {
            self.modal = Some(m);
        } else {
            self.modal_queue.push_back(m);
        }
    }

    fn close_modal(&mut self) {
        self.modal = self.modal_queue.pop_front();
    }

    pub fn request_permission(
        &mut self,
        req: RequestPermissionRequest,
        reply: oneshot::Sender<RequestPermissionOutcome>,
    ) {
        let title = req
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "Permission required".into());
        let summary = req
            .tool_call
            .fields
            .raw_input
            .as_ref()
            .map(summarize_input)
            .unwrap_or_default();
        self.push_modal(Modal::Permission(PermissionModal {
            title,
            summary,
            options: req.options,
            selected: 0,
            reply: Some(reply),
        }));
    }

    pub fn ask_questions(&mut self, req: AskRequest, reply: oneshot::Sender<AskResponse>) {
        if req.questions.is_empty() {
            let _ = reply.send(AskResponse::default());
            return;
        }
        let states = req
            .questions
            .iter()
            .map(|q| QState {
                checked: vec![false; q.options.len() + 1],
                ..Default::default()
            })
            .collect();
        self.push_modal(Modal::Ask(AskModal {
            questions: req.questions,
            page: 0,
            states,
            reply: Some(reply),
        }));
    }

    pub fn show_sessions(&mut self, sessions: Vec<SessionEntry>) {
        if sessions.is_empty() {
            self.notice("no previous sessions");
            return;
        }
        self.push_modal(Modal::Picker(PickerModal {
            sessions,
            selected: 0,
        }));
    }

    // ---- turns ------------------------------------------------------------

    /// Record a user prompt and mark the agent busy.
    pub fn begin_turn(&mut self, text: String) -> String {
        self.blocks.push(Block::User(text.clone()));
        self.running = true;
        self.scroll = 0;
        text
    }

    pub fn end_turn(&mut self, err: Option<String>, stop: Option<&str>) {
        self.running = false;
        if let Some(e) = err {
            self.error(e);
        } else if let Some(s) = stop
            && s != "end_turn"
        {
            self.notice(format!("stopped: {s}"));
        }
    }

    pub fn next_queued(&mut self) -> Option<String> {
        if self.running {
            return None;
        }
        let t = self.queue.pop_front()?;
        Some(self.begin_turn(t))
    }

    pub fn approx_tokens(&self) -> usize {
        self.chars_streamed / 4
    }

    /// Status-bar token text: real usage when the agent reports it, else a rough estimate.
    pub fn tokens_label(&self) -> String {
        match self.usage {
            Some((used, size)) if size > 0 => {
                format!("{} / {} tok", compact_count(used), compact_count(size))
            }
            Some((used, _)) => format!("{} tok", compact_count(used)),
            None => format!("~{} tok", self.approx_tokens()),
        }
    }

    // ---- input ------------------------------------------------------------

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_by(3),
            MouseEventKind::ScrollDown => self.scroll_by(-3),
            _ => {}
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll.get();
        let cur = self.scroll.min(max) as isize;
        self.scroll = (cur + delta).clamp(0, max as isize) as usize;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if ctrl && key.code == KeyCode::Char('c') {
            if !self.input.is_empty() {
                self.input.clear();
                self.cursor = 0;
                self.quit_armed = false;
                return Action::None;
            }
            if self.quit_armed {
                self.should_quit = true;
                return Action::Quit;
            }
            self.quit_armed = true;
            self.notice("press ctrl+c again to quit");
            return Action::None;
        }
        self.quit_armed = false;

        match key.code {
            KeyCode::Esc if self.running => return Action::Cancel,
            KeyCode::BackTab => return self.cycle_mode(),
            KeyCode::Tab if shift => return self.cycle_mode(),
            KeyCode::Char('t') if ctrl => self.show_thoughts = !self.show_thoughts,
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::PageUp => self.scroll_by(self.page_height.get().max(2) as isize - 1),
            KeyCode::PageDown => self.scroll_by(-(self.page_height.get().max(2) as isize - 1)),
            KeyCode::Enter if shift || alt => self.insert('\n'),
            KeyCode::Enter => return self.submit(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => {
                if let Some(c) = self.input[self.cursor..].chars().next() {
                    self.input.drain(self.cursor..self.cursor + c.len_utf8());
                }
            }
            KeyCode::Left => {
                if let Some(c) = self.input[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.input[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => {
                self.cursor = self.input[..self.cursor]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0)
            }
            KeyCode::End => {
                self.cursor = self.input[self.cursor..]
                    .find('\n')
                    .map(|i| self.cursor + i)
                    .unwrap_or(self.input.len())
            }
            KeyCode::Up => self.up_down(true),
            KeyCode::Down => self.up_down(false),
            _ => {}
        }
        Action::None
    }

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.hist_pos = None;
    }

    fn backspace(&mut self) {
        if let Some(c) = self.input[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.input.drain(self.cursor..self.cursor + c.len_utf8());
        }
    }

    fn up_down(&mut self, up: bool) {
        if self.input.is_empty() || self.hist_pos.is_some() {
            if self.history.is_empty() {
                return;
            }
            let new = match (self.hist_pos, up) {
                (None, true) => Some(self.history.len() - 1),
                (Some(0), true) => Some(0),
                (Some(i), true) => Some(i - 1),
                (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
                (Some(_), false) => None,
                (None, false) => None,
            };
            self.hist_pos = new;
            self.input = new.map(|i| self.history[i].clone()).unwrap_or_default();
            self.cursor = self.input.len();
            return;
        }
        // Move between lines of a multi-line draft.
        let line_start = self.input[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let col = self.input[line_start..self.cursor].chars().count();
        let target_start = if up {
            if line_start == 0 {
                return;
            }
            self.input[..line_start - 1]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0)
        } else {
            match self.input[self.cursor..].find('\n') {
                Some(i) => self.cursor + i + 1,
                None => return,
            }
        };
        let target_end = self.input[target_start..]
            .find('\n')
            .map(|i| target_start + i)
            .unwrap_or(self.input.len());
        let off: usize = self.input[target_start..target_end]
            .chars()
            .take(col)
            .map(char::len_utf8)
            .sum();
        self.cursor = target_start + off;
    }

    fn cycle_mode(&mut self) -> Action {
        if self.modes.is_empty() {
            self.notice("no modes available");
            return Action::None;
        }
        let i = self
            .modes
            .iter()
            .position(|m| *m.id.0 == *self.mode_id)
            .map(|i| (i + 1) % self.modes.len())
            .unwrap_or(0);
        Action::SetMode(self.modes[i].id.to_string())
    }

    fn submit(&mut self) -> Action {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return Action::None;
        }
        self.input.clear();
        self.cursor = 0;
        self.hist_pos = None;
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        if let Some(cmd) = text.strip_prefix('/') {
            return self.slash(cmd);
        }
        if self.running {
            self.queue.push_back(text);
            self.notice("queued; will send when the agent is done");
            return Action::None;
        }
        Action::Submit(self.begin_turn(text))
    }

    fn slash(&mut self, cmd: &str) -> Action {
        let (name, arg) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };
        match name {
            "help" | "?" => self.notice(HELP),
            "mode" => {
                if arg.is_empty() {
                    self.notice(self.modes_text());
                } else if self.modes.iter().any(|m| &*m.id.0 == arg) {
                    return Action::SetMode(arg.to_string());
                } else {
                    self.error(format!("unknown mode `{arg}`"));
                    self.notice(self.modes_text());
                }
            }
            "modes" => self.notice(self.modes_text()),
            "setup" => return Action::Setup,
            "model" => {
                if arg.is_empty() {
                    if self.models.is_empty() {
                        self.notice(format!(
                            "no models are listed for this profile (current: {}); use `/model <id>` or run /setup",
                            self.model_label
                        ));
                    } else {
                        self.push_modal(Modal::Models(ModelPickerModal {
                            models: self.models.clone(),
                            selected: 0,
                        }));
                    }
                } else if arg == "default" {
                    return Action::SetModel(None);
                } else {
                    return Action::SetModel(Some(arg.to_string()));
                }
            }
            "clear" | "new" => return Action::NewSession,
            "resume" | "sessions" => return Action::ListSessions,
            "init" => self.notice(
                "Run `ez init` in your shell to generate .ez/modes.toml (add --minimal for plan/build/verify).",
            ),
            "quit" | "exit" | "q" => {
                self.should_quit = true;
                return Action::Quit;
            }
            other => self.error(format!("unknown command /{other}; try /help")),
        }
        Action::None
    }

    fn modes_text(&self) -> String {
        if self.modes.is_empty() {
            return "no modes available; run `ez init`".into();
        }
        let mut s = format!("current mode: {}\n", self.mode_id);
        for m in &self.modes {
            s.push_str(&format!(
                "  {} {}{}\n",
                if *m.id.0 == *self.mode_id { "*" } else { " " },
                m.id,
                m.description
                    .as_ref()
                    .map(|d| format!(" - {d}"))
                    .unwrap_or_default()
            ));
        }
        s.trim_end().to_string()
    }

    // ---- modals -------------------------------------------------------------

    fn modal_key(&mut self, key: KeyEvent) -> Action {
        let Some(modal) = self.modal.take() else {
            return Action::None;
        };
        let (keep, action) = match modal {
            Modal::Permission(mut m) => {
                let done = permission_key(&mut m, key);
                (
                    if done {
                        None
                    } else {
                        Some(Modal::Permission(m))
                    },
                    Action::None,
                )
            }
            Modal::Ask(mut m) => {
                let done = ask_key(&mut m, key);
                (if done { None } else { Some(Modal::Ask(m)) }, Action::None)
            }
            Modal::Models(mut p) => {
                let n = p.models.len() + 1;
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => p.selected = (p.selected + n - 1) % n,
                    KeyCode::Down | KeyCode::Char('j') => p.selected = (p.selected + 1) % n,
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.modal = None;
                        self.close_modal();
                        return Action::None;
                    }
                    KeyCode::Enter => {
                        let choice = p.selected.checked_sub(1).map(|i| p.models[i].clone());
                        self.modal = None;
                        self.close_modal();
                        return Action::SetModel(choice);
                    }
                    _ => {}
                }
                (Some(Modal::Models(p)), Action::None)
            }
            Modal::Picker(mut p) => match picker_key(&mut p, key) {
                PickerResult::Stay => (Some(Modal::Picker(p)), Action::None),
                PickerResult::Close => (
                    None,
                    if self.session_id.is_none() {
                        Action::NewSession
                    } else {
                        Action::None
                    },
                ),
                PickerResult::Load(id) => (None, Action::LoadSession(id)),
            },
        };
        self.modal = keep;
        if self.modal.is_none() {
            self.close_modal();
        }
        action
    }
}

/// 1234 -> "1.2k", 200000 -> "200k", 1500000 -> "1.5M".
fn compact_count(n: u64) -> String {
    let fmt = |v: f64, unit: &str| {
        let s = format!("{v:.1}");
        format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
    };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => fmt(n as f64 / 1e3, "k"),
        _ => fmt(n as f64 / 1e6, "M"),
    }
}

fn apply_content(v: &mut ToolView, content: &[ToolCallContent]) {
    for c in content {
        match c {
            ToolCallContent::Content(c) => {
                if let ContentBlock::Text(t) = &c.content {
                    v.text.extend(t.text.lines().map(str::to_string));
                }
            }
            ToolCallContent::Diff(d) => {
                v.diff
                    .push((DiffKind::Hunk, format!("--- {}", d.path.display())));
                let old = d.old_text.as_deref().unwrap_or("");
                let d = similar::TextDiff::from_lines(old, &d.new_text);
                for ch in d.iter_all_changes() {
                    let line = ch.value().trim_end_matches('\n').to_string();
                    match ch.tag() {
                        similar::ChangeTag::Insert => {
                            v.diff.push((DiffKind::Add, format!("+{line}")))
                        }
                        similar::ChangeTag::Delete => {
                            v.diff.push((DiffKind::Del, format!("-{line}")))
                        }
                        similar::ChangeTag::Equal => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', "⏎");
    if flat.chars().count() > max {
        let t: String = flat.chars().take(max).collect();
        format!("{t}…")
    } else {
        flat
    }
}

pub fn summarize_input(v: &Value) -> Vec<String> {
    match v {
        Value::Object(map) => map
            .iter()
            .take(8)
            .map(|(k, val)| {
                let s = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}: {}", truncate(&s, 300))
            })
            .collect(),
        Value::Null => vec![],
        other => vec![truncate(&other.to_string(), 300)],
    }
}

/// Returns true when the modal is finished.
fn permission_key(m: &mut PermissionModal, key: KeyEvent) -> bool {
    let n = m.options.len();
    let choose = |m: &mut PermissionModal, outcome: RequestPermissionOutcome| {
        if let Some(tx) = m.reply.take() {
            let _ = tx.send(outcome);
        }
        true
    };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if n > 0 {
                m.selected = (m.selected + n - 1) % n;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            if n > 0 {
                m.selected = (m.selected + 1) % n;
            }
            false
        }
        KeyCode::Enter => match m.options.get(m.selected) {
            Some(o) => {
                let id = o.option_id.clone();
                choose(
                    m,
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                )
            }
            None => choose(m, RequestPermissionOutcome::Cancelled),
        },
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let i = c as usize - '1' as usize;
            match m.options.get(i) {
                Some(o) => {
                    let id = o.option_id.clone();
                    choose(
                        m,
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                    )
                }
                None => false,
            }
        }
        KeyCode::Esc => {
            let outcome = m
                .options
                .iter()
                .find(|o| o.kind == PermissionOptionKind::RejectOnce)
                .map(|o| {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(RequestPermissionOutcome::Cancelled);
            choose(m, outcome)
        }
        _ => false,
    }
}

impl AskModal {
    pub fn answers(&self) -> Vec<Answer> {
        self.questions
            .iter()
            .zip(&self.states)
            .map(|(q, st)| {
                let n = q.options.len();
                let selected = q
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| st.checked[*i])
                    .map(|(_, o)| o.label.clone())
                    .collect();
                let other = (st.checked[n] && !st.other_text.trim().is_empty())
                    .then(|| st.other_text.trim().to_string());
                Answer {
                    question: q.question.clone(),
                    selected,
                    other,
                }
            })
            .collect()
    }

    fn finish(&mut self, cancelled: bool) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.send(if cancelled {
                AskResponse::cancelled()
            } else {
                AskResponse {
                    answers: self.answers(),
                    cancelled: false,
                }
            });
        }
    }

    /// Select/toggle the option under the cursor. Returns true if free text editing began.
    fn pick(&mut self) {
        let multi = self.questions[self.page].multi_select;
        let st = &mut self.states[self.page];
        let other = st.checked.len() - 1;
        let i = st.cursor;
        if multi {
            st.checked[i] = !st.checked[i];
        } else {
            st.checked.iter_mut().for_each(|c| *c = false);
            st.checked[i] = true;
        }
        if i == other {
            st.editing = st.checked[i];
        }
    }

    /// Go to the next page; returns true if this was the last one.
    fn advance(&mut self) -> bool {
        if self.page + 1 < self.questions.len() {
            self.page += 1;
            false
        } else {
            true
        }
    }
}

/// Returns true when the modal is finished.
fn ask_key(m: &mut AskModal, key: KeyEvent) -> bool {
    let page = m.page;
    let n_opts = m.questions[page].options.len() + 1;
    let multi = m.questions[page].multi_select;

    if m.states[page].editing {
        let st = &mut m.states[page];
        let other = n_opts - 1;
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                st.other_text.push(c)
            }
            KeyCode::Backspace => {
                st.other_text.pop();
            }
            KeyCode::Esc => {
                st.editing = false;
                if st.other_text.trim().is_empty() {
                    st.checked[other] = false;
                }
            }
            KeyCode::Enter => {
                st.editing = false;
                if st.other_text.trim().is_empty() {
                    st.checked[other] = false;
                } else if !multi {
                    // Free text answers a single-select question outright.
                    if m.advance() {
                        m.finish(false);
                        return true;
                    }
                }
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Esc => {
            m.finish(true);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let st = &mut m.states[page];
            st.cursor = (st.cursor + n_opts - 1) % n_opts;
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let st = &mut m.states[page];
            st.cursor = (st.cursor + 1) % n_opts;
            false
        }
        KeyCode::Left | KeyCode::BackTab => {
            m.page = m.page.saturating_sub(1);
            false
        }
        KeyCode::Right | KeyCode::Tab => {
            if m.page + 1 < m.questions.len() {
                m.page += 1;
            }
            false
        }
        KeyCode::Char(' ') => {
            m.pick();
            false
        }
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let i = c as usize - '1' as usize;
            if i < n_opts {
                m.states[page].cursor = i;
                m.pick();
            }
            false
        }
        KeyCode::Enter => {
            let st = &m.states[page];
            let other = n_opts - 1;
            if multi {
                // Enter on an unchecked "Other…" opens the text box; otherwise confirm the page.
                if st.cursor == other && !st.checked[other] {
                    m.pick();
                    return false;
                }
            } else if !st.checked.iter().any(|c| *c) {
                m.pick();
                if m.states[page].editing {
                    return false;
                }
            }
            if m.advance() {
                m.finish(false);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

enum PickerResult {
    Stay,
    Close,
    Load(String),
}

fn picker_key(p: &mut PickerModal, key: KeyEvent) -> PickerResult {
    let n = p.sessions.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            p.selected = (p.selected + n - 1) % n;
            PickerResult::Stay
        }
        KeyCode::Down | KeyCode::Char('j') => {
            p.selected = (p.selected + 1) % n;
            PickerResult::Stay
        }
        KeyCode::Enter => PickerResult::Load(p.sessions[p.selected].id.clone()),
        KeyCode::Esc | KeyCode::Char('q') => PickerResult::Close,
        _ => PickerResult::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::QuestionOption;
    use crossterm::event::KeyEventKind;
    use ez_acp::schema::{
        CurrentModeUpdate, Diff, Notice, NoticeSeverity, Plan, PlanEntryPriority, PlanEntryStatus,
        ToolCallUpdateFields, UsageUpdate,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn typ(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }
    fn text_chunk(s: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(s)))
    }
    fn mode(id: &str) -> SessionMode {
        SessionMode::new(id.to_string(), id)
    }

    #[test]
    fn streams_merge_into_one_block() {
        let mut a = App::new("m");
        a.handle_update(text_chunk("hel"));
        a.handle_update(text_chunk("lo"));
        assert_eq!(a.blocks.len(), 1);
        assert!(matches!(&a.blocks[0], Block::Agent(s) if s == "hello"));
        a.handle_update(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::from("hmm"),
        )));
        assert_eq!(a.blocks.len(), 2);
    }

    #[test]
    fn tool_call_lifecycle() {
        let mut a = App::new("m");
        a.handle_update(SessionUpdate::ToolCall(
            ToolCall::new("c1", "Read a.rs")
                .kind(ToolKind::Read)
                .status(ToolCallStatus::Pending),
        ));
        a.handle_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "c1",
            ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
        )));
        a.handle_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "c1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![
                    ToolCallContent::from("l1\nl2"),
                    ToolCallContent::Diff(Diff::new("a.rs", "y\n").old_text("x\n")),
                ]),
        )));
        assert_eq!(a.blocks.len(), 1);
        let Block::Tool(t) = &a.blocks[0] else {
            panic!()
        };
        assert_eq!(t.status, ToolCallStatus::Completed);
        assert_eq!(t.title, "Read a.rs");
        assert_eq!(t.text, vec!["l1", "l2"]);
        assert!(t.diff.contains(&(DiffKind::Add, "+y".into())));
        assert!(t.diff.contains(&(DiffKind::Del, "-x".into())));
        // Update for an unknown id creates a card.
        a.handle_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "zz",
            ToolCallUpdateFields::new().status(ToolCallStatus::Failed),
        )));
        assert_eq!(a.blocks.len(), 2);
    }

    #[test]
    fn plan_and_mode_updates() {
        let mut a = App::new("m");
        a.handle_update(SessionUpdate::Plan(Plan::new(vec![PlanEntry::new(
            "do it",
            PlanEntryPriority::High,
            PlanEntryStatus::Pending,
        )])));
        assert_eq!(a.plan.len(), 1);
        a.handle_update(SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(
            "test",
        )));
        assert_eq!(a.mode_id, "test");
        // Notices arrive as their own update and usage drives the status-bar token count.
        a.handle_update(SessionUpdate::Notice(Notice::new(
            NoticeSeverity::Info,
            "heads up",
        )));
        assert!(matches!(a.blocks.last(), Some(Block::Notice(s)) if s == "heads up"));
        assert!(a.tokens_label().starts_with('~'));
        a.handle_update(SessionUpdate::UsageUpdate(UsageUpdate::new(1234, 200_000)));
        assert_eq!(a.tokens_label(), "1.2k / 200k tok");
    }

    #[test]
    fn typing_submit_history_and_newline() {
        let mut a = App::new("m");
        typ(&mut a, "héllo");
        assert_eq!(a.handle_key(key(KeyCode::Left)), Action::None);
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(a.input, "héll\no");
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.input, "héllo");
        a.cursor = a.input.len();
        let act = a.handle_key(key(KeyCode::Enter));
        assert_eq!(act, Action::Submit("héllo".into()));
        assert!(a.running && a.input.is_empty());
        // while running, prompts queue
        typ(&mut a, "next");
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(a.queue.len(), 1);
        a.end_turn(None, Some("end_turn"));
        assert_eq!(a.next_queued().as_deref(), Some("next"));
        a.end_turn(None, Some("end_turn"));
        // history
        a.handle_key(key(KeyCode::Up));
        assert_eq!(a.input, "next");
        a.handle_key(key(KeyCode::Up));
        assert_eq!(a.input, "héllo");
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.input, "");
    }

    #[test]
    fn esc_cancels_only_when_running_and_ctrl_c_twice_quits() {
        let mut a = App::new("m");
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Action::None);
        a.running = true;
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Action::Cancel);
        let cc = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(a.handle_key(cc), Action::None);
        assert!(a.quit_armed);
        assert_eq!(a.handle_key(cc), Action::Quit);
        assert!(a.should_quit);
        let _ = KeyEventKind::Press;
    }

    #[test]
    fn mode_cycling_and_slash_commands() {
        let mut a = App::new("m");
        a.set_modes(vec![mode("a"), mode("b")], "a".into());
        assert_eq!(
            a.handle_key(key(KeyCode::BackTab)),
            Action::SetMode("b".into())
        );
        a.mode_id = "b".into();
        assert_eq!(
            a.handle_key(key(KeyCode::BackTab)),
            Action::SetMode("a".into())
        );
        typ(&mut a, "/mode b");
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Action::SetMode("b".into())
        );
        typ(&mut a, "/mode zzz");
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::None);
        assert!(matches!(
            a.blocks.iter().rev().nth(1),
            Some(Block::Error(_))
        ));
        typ(&mut a, "/clear");
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::NewSession);
        typ(&mut a, "/resume");
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::ListSessions);
        typ(&mut a, "/quit");
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::Quit);
        assert!(!a.running);
    }

    #[test]
    fn permission_modal_select_and_escape() {
        let opts = vec![
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
        ];
        let req = |opts: Vec<PermissionOption>| {
            RequestPermissionRequest::new(
                "s",
                ToolCallUpdate::new(
                    "c",
                    ToolCallUpdateFields::new()
                        .title("Bash: ls")
                        .raw_input(serde_json::json!({"command":"ls"})),
                ),
                opts,
            )
        };
        let mut a = App::new("m");
        let (tx, mut rx) = oneshot::channel();
        a.request_permission(req(opts.clone()), tx);
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Enter));
        assert!(a.modal.is_none());
        assert_eq!(
            rx.try_recv().unwrap(),
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("deny"))
        );
        let (tx, mut rx) = oneshot::channel();
        a.request_permission(req(opts.clone()), tx);
        a.handle_key(key(KeyCode::Char('1')));
        assert_eq!(
            rx.try_recv().unwrap(),
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow"))
        );
        let (tx, mut rx) = oneshot::channel();
        a.request_permission(req(opts), tx);
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(
            rx.try_recv().unwrap(),
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("deny"))
        );
        let (tx, mut rx) = oneshot::channel();
        a.request_permission(req(vec![]), tx);
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(rx.try_recv().unwrap(), RequestPermissionOutcome::Cancelled);
    }

    fn q(text: &str, multi: bool, labels: &[&str]) -> Question {
        Question {
            question: text.into(),
            header: "h".into(),
            options: labels
                .iter()
                .map(|l| QuestionOption {
                    label: (*l).into(),
                    description: format!("about {l}"),
                })
                .collect(),
            multi_select: multi,
        }
    }

    #[test]
    fn ask_modal_single_multi_and_other() {
        let mut a = App::new("m");
        let (tx, mut rx) = oneshot::channel();
        a.ask_questions(
            AskRequest {
                session_id: "s".into(),
                questions: vec![
                    q("Pick one", false, &["A", "B"]),
                    q("Pick many", true, &["X", "Y", "Z"]),
                    q("Pick or type", false, &["P"]),
                ],
            },
            tx,
        );
        // Q1: move to B, Enter selects and advances.
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Enter));
        // Q2: toggle X and Z with space, Enter advances.
        a.handle_key(key(KeyCode::Char(' ')));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Char(' ')));
        a.handle_key(key(KeyCode::Enter));
        // Q3: choose Other (second row) and type.
        a.handle_key(key(KeyCode::Char('2')));
        for c in "custom".chars() {
            a.handle_key(key(KeyCode::Char(c)));
        }
        a.handle_key(key(KeyCode::Enter));
        assert!(a.modal.is_none());
        let r = rx.try_recv().unwrap();
        assert!(!r.cancelled);
        assert_eq!(r.answers[0].selected, vec!["B"]);
        assert_eq!(r.answers[1].selected, vec!["X", "Z"]);
        assert!(r.answers[2].selected.is_empty());
        assert_eq!(r.answers[2].other.as_deref(), Some("custom"));
    }

    #[test]
    fn ask_modal_multi_other_and_cancel() {
        let mut a = App::new("m");
        let (tx, mut rx) = oneshot::channel();
        a.ask_questions(
            AskRequest {
                session_id: "s".into(),
                questions: vec![q("many", true, &["X"])],
            },
            tx,
        );
        a.handle_key(key(KeyCode::Char('1')));
        a.handle_key(key(KeyCode::Char('2'))); // Other: starts editing
        for c in "zz".chars() {
            a.handle_key(key(KeyCode::Char(c)));
        }
        a.handle_key(key(KeyCode::Enter)); // finish text
        assert!(a.modal.is_some());
        a.handle_key(key(KeyCode::Enter)); // confirm
        let r = rx.try_recv().unwrap();
        assert_eq!(r.answers[0].selected, vec!["X"]);
        assert_eq!(r.answers[0].other.as_deref(), Some("zz"));

        let (tx, mut rx) = oneshot::channel();
        a.ask_questions(
            AskRequest {
                session_id: "s".into(),
                questions: vec![q("one", false, &["X"])],
            },
            tx,
        );
        a.handle_key(key(KeyCode::Esc));
        assert!(rx.try_recv().unwrap().cancelled);
    }

    /// The demo mock model's `mock:ask` request, as the UI receives it over the wire.
    #[test]
    fn demo_ask_request_roundtrip() {
        let req: AskRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s",
            "questions": [
                {"question": "Which approach should I take?", "header": "Approach",
                 "multiSelect": false,
                 "options": [{"label": "Incremental (Recommended)", "description": "d"},
                             {"label": "Rewrite", "description": "d"},
                             {"label": "Leave as is", "description": "d"}]},
                {"question": "Which checks should I run?", "header": "Checks",
                 "multiSelect": true,
                 "options": [{"label": "Unit tests"}, {"label": "Lints"}, {"label": "Formatting"}]}
            ]
        }))
        .unwrap();
        let mut a = App::new("m");
        let (tx, mut rx) = oneshot::channel();
        a.ask_questions(req, tx);
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Enter)); // Q1: Rewrite
        a.handle_key(key(KeyCode::Char(' '))); // Q2: Unit tests
        a.handle_key(key(KeyCode::Char('4'))); // Other: edit
        for c in "note".chars() {
            a.handle_key(key(KeyCode::Char(c)));
        }
        a.handle_key(key(KeyCode::Enter)); // finish text
        a.handle_key(key(KeyCode::Enter)); // confirm
        assert!(a.modal.is_none());
        let v = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(v["cancelled"], false);
        assert_eq!(v["answers"][0]["selected"], serde_json::json!(["Rewrite"]));
        assert_eq!(
            v["answers"][1]["selected"],
            serde_json::json!(["Unit tests"])
        );
        assert_eq!(v["answers"][1]["other"], "note");
    }

    #[test]
    fn demo_write_permission_request() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s",
            "toolCall": {"toolCallId": "c", "title": "Write a.txt",
                         "rawInput": {"file_path": "a.txt", "content": "hello"}},
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]
        }))
        .unwrap();
        let mut a = App::new("m");
        let (tx, mut rx) = oneshot::channel();
        a.request_permission(req, tx);
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow"))
        );
    }

    #[test]
    fn modals_queue_in_order() {
        let mut a = App::new("m");
        let (t1, mut r1) = oneshot::channel();
        let (t2, mut r2) = oneshot::channel();
        for t in [t1, t2] {
            a.ask_questions(
                AskRequest {
                    session_id: "s".into(),
                    questions: vec![q("x", false, &["A"])],
                },
                t,
            );
        }
        a.handle_key(key(KeyCode::Esc));
        assert!(r1.try_recv().unwrap().cancelled);
        assert!(a.modal.is_some());
        assert!(r2.try_recv().is_err());
        a.handle_key(key(KeyCode::Esc));
        assert!(r2.try_recv().unwrap().cancelled);
        assert!(a.modal.is_none());
    }

    #[test]
    fn picker_loads_or_starts_new() {
        let mut a = App::new("m");
        let s = |id: &str| SessionEntry {
            id: id.into(),
            ..Default::default()
        };
        a.show_sessions(vec![s("one"), s("two")]);
        a.handle_key(key(KeyCode::Down));
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Action::LoadSession("two".into())
        );
        a.show_sessions(vec![s("one")]);
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Action::NewSession);
        a.session_id = Some("x".into());
        a.show_sessions(vec![s("one")]);
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Action::None);
    }

    #[test]
    fn scroll_is_clamped() {
        let mut a = App::new("m");
        a.max_scroll.set(5);
        a.scroll_by(100);
        assert_eq!(a.scroll, 5);
        a.scroll_by(-100);
        assert_eq!(a.scroll, 0);
    }
}
