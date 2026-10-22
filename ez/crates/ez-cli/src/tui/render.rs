//! Rendering: `App` -> ratatui widgets. Pure with respect to the terminal.

use ez_acp::schema::{PlanEntryStatus, ToolCallStatus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, Borders, Clear, Paragraph};

use super::app::{App, AskModal, Block, DiffKind, Modal, PermissionModal, PickerModal, ToolView};

const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];
const TOOL_PREVIEW_LINES: usize = 8;

fn spinner(app: &App) -> &'static str {
    SPINNER[(app.tick / 2) as usize % SPINNER.len()]
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn mode_color(mode: &str) -> Color {
    match mode {
        "explore" | "plan" => Color::Cyan,
        "design" => Color::Magenta,
        "implement" | "build" => Color::Green,
        "test" | "verify" => Color::Yellow,
        "review" => Color::Blue,
        "release" => Color::Red,
        _ => Color::Gray,
    }
}

// ---- wrapping ---------------------------------------------------------------

/// Hard/soft wrap one styled line to `width` columns (counting chars).
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    if chars.len() <= width {
        return vec![line];
    }
    let mut out = vec![];
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + width).min(chars.len());
        if end < chars.len()
            && let Some(sp) = chars[start..end].iter().rposition(|(c, _)| *c == ' ')
            && sp > 0
        {
            end = start + sp + 1;
        }
        out.push(spans_of(&chars[start..end]));
        start = end;
    }
    out
}

fn spans_of(chars: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![];
    let mut cur = String::new();
    let mut style = chars.first().map(|c| c.1).unwrap_or_default();
    for (c, s) in chars {
        if *s != style && !cur.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut cur), style));
        }
        style = *s;
        cur.push(*c);
    }
    if !cur.is_empty() {
        spans.push(Span::styled(cur, style));
    }
    Line::from(spans)
}

/// Rows of the input box and the cursor position within them.
pub fn input_layout(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = vec![];
    let mut cpos = (0, 0);
    let mut offset = 0;
    for (li, line) in text.split('\n').enumerate() {
        let _ = li;
        let chars: Vec<char> = line.chars().collect();
        let first_row = rows.len();
        if chars.is_empty() {
            rows.push(String::new());
        }
        for chunk in chars.chunks(width) {
            rows.push(chunk.iter().collect());
        }
        let line_end = offset + line.len();
        if cursor >= offset && cursor <= line_end {
            let col = text[offset..cursor].chars().count();
            let r = first_row + col / width;
            // Cursor at the end of a full row wraps to a new empty row.
            if rows.len() <= r {
                rows.push(String::new());
            }
            cpos = (r, col % width);
        }
        offset = line_end + 1;
    }
    (rows, cpos)
}

// ---- transcript -------------------------------------------------------------

fn tool_glyph(app: &App, s: ToolCallStatus) -> (String, Style) {
    match s {
        ToolCallStatus::Pending => ("○".into(), dim()),
        ToolCallStatus::InProgress => (spinner(app).into(), Style::default().fg(Color::Yellow)),
        ToolCallStatus::Completed => ("✓".into(), Style::default().fg(Color::Green)),
        ToolCallStatus::Failed => ("✗".into(), Style::default().fg(Color::Red)),
        _ => ("?".into(), dim()),
    }
}

fn tool_lines(app: &App, t: &ToolView, out: &mut Vec<Line<'static>>) {
    let (glyph, gstyle) = tool_glyph(app, t.status);
    out.push(Line::from(vec![
        Span::styled("● ", gstyle),
        Span::styled(
            t.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(glyph, gstyle),
    ]));
    let mut body: Vec<(String, Style)> = t.text.iter().map(|l| (l.clone(), dim())).collect();
    for (k, l) in &t.diff {
        let style = match k {
            DiffKind::Add => Style::default().fg(Color::Green),
            DiffKind::Del => Style::default().fg(Color::Red),
            DiffKind::Hunk => dim(),
        };
        body.push((l.clone(), style));
    }
    let total = body.len();
    for (l, style) in body.into_iter().take(TOOL_PREVIEW_LINES) {
        out.push(Line::from(vec![
            Span::styled("  │ ", dim()),
            Span::styled(l, style),
        ]));
    }
    if total > TOOL_PREVIEW_LINES {
        out.push(Line::from(Span::styled(
            format!("  │ … +{} more lines", total - TOOL_PREVIEW_LINES),
            dim(),
        )));
    }
}

fn agent_lines(text: &str, out: &mut Vec<Line<'static>>) {
    let mut in_fence = false;
    for l in text.split('\n') {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(Line::from(Span::styled(l.to_string(), dim())));
        } else if in_fence {
            out.push(Line::from(Span::styled(format!("  {l}"), dim())));
        } else {
            out.push(Line::from(l.to_string()));
        }
    }
}

pub fn transcript_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut raw: Vec<Line<'static>> = vec![];
    if app.blocks.is_empty() {
        raw.push(Line::from(Span::styled(
            "ez — type a message and press Enter. /help lists commands.",
            dim(),
        )));
        if app.modes.len() <= 1 {
            raw.push(Line::from(Span::styled(
                "No modes configured: run `ez init` to generate .ez/modes.toml.",
                dim(),
            )));
        }
    }
    let last = app.blocks.len().saturating_sub(1);
    for (i, b) in app.blocks.iter().enumerate() {
        match b {
            Block::User(t) => {
                for (n, l) in t.split('\n').enumerate() {
                    let prefix = if n == 0 { "› " } else { "  " };
                    raw.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Cyan)),
                        Span::styled(l.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                    ]));
                }
            }
            Block::Agent(t) => agent_lines(t, &mut raw),
            Block::Thought(t) => {
                if app.show_thoughts {
                    for (n, l) in t.split('\n').enumerate() {
                        let prefix = if n == 0 { "✻ " } else { "  " };
                        raw.push(Line::from(Span::styled(
                            format!("{prefix}{l}"),
                            dim().add_modifier(Modifier::ITALIC),
                        )));
                    }
                } else {
                    let active = app.running && i == last;
                    raw.push(Line::from(Span::styled(
                        if active {
                            "✻ thinking… (ctrl+t to expand)"
                        } else {
                            "✻ thought (ctrl+t to expand)"
                        },
                        dim().add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            Block::Tool(t) => tool_lines(app, t, &mut raw),
            Block::Notice(t) => {
                for l in t.split('\n') {
                    raw.push(Line::from(Span::styled(
                        l.to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::DIM),
                    )));
                }
            }
            Block::Error(t) => {
                for l in t.split('\n') {
                    raw.push(Line::from(Span::styled(
                        format!("error: {l}"),
                        Style::default().fg(Color::Red),
                    )));
                }
            }
        }
        // Breathing room between prose blocks, but keep tool cards tight.
        if !matches!(b, Block::Tool(_) | Block::Thought(_)) {
            raw.push(Line::default());
        }
    }
    raw.into_iter().flat_map(|l| wrap_line(l, width)).collect()
}

// ---- frame --------------------------------------------------------------------

pub fn render(app: &App, f: &mut Frame) {
    let area = f.area();
    let (rows, cpos) = {
        let w = (area.width as usize).saturating_sub(4);
        input_layout(&app.input, app.cursor, w)
    };
    let input_h = (rows.len().clamp(1, 6) + 2) as u16;
    let plan_h = if app.plan.is_empty() {
        0
    } else {
        (app.plan.len().min(6) + 2) as u16
    };
    let chunks = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(plan_h),
        Constraint::Length(input_h),
        Constraint::Length(1),
    ])
    .split(area);

    render_transcript(app, f, chunks[0]);
    if plan_h > 0 {
        render_plan(app, f, chunks[1]);
    }
    render_input(app, f, chunks[2], &rows, cpos);
    render_status(app, f, chunks[3]);
    if let Some(m) = &app.modal {
        render_modal(m, f, area);
    }
}

fn render_transcript(app: &App, f: &mut Frame, area: Rect) {
    let lines = transcript_lines(app, area.width.max(1) as usize);
    let h = area.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    app.max_scroll.set(max_scroll);
    app.page_height.set(h);
    let scroll = app.scroll.min(max_scroll);
    let start = max_scroll - scroll;
    let visible: Vec<Line> = lines.into_iter().skip(start).take(h).collect();
    f.render_widget(Paragraph::new(visible), area);
}

fn render_plan(app: &App, f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = app
        .plan
        .iter()
        .take(6)
        .map(|e| {
            let (g, style) = match e.status {
                PlanEntryStatus::Pending => ("☐", dim()),
                PlanEntryStatus::InProgress => ("◐", Style::default().fg(Color::Yellow)),
                PlanEntryStatus::Completed => (
                    "☑",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::CROSSED_OUT),
                ),
                _ => ("☐", dim()),
            };
            Line::from(vec![
                Span::styled(format!("{g} "), style),
                Span::styled(e.content.clone(), style),
            ])
        })
        .collect();
    let done = app
        .plan
        .iter()
        .filter(|e| e.status == PlanEntryStatus::Completed)
        .count();
    let block = UiBlock::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(format!(" plan {}/{} ", done, app.plan.len()));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_input(app: &App, f: &mut Frame, area: Rect, rows: &[String], cpos: (usize, usize)) {
    let inner_h = area.height.saturating_sub(2) as usize;
    // Keep the cursor row visible when the draft is taller than the box.
    let first = (cpos.0 + 1).saturating_sub(inner_h);
    let lines: Vec<Line> = rows
        .iter()
        .skip(first)
        .take(inner_h)
        .enumerate()
        .map(|(i, r)| {
            if first + i == 0 {
                Line::from(vec![
                    Span::styled("› ", Style::default().fg(Color::Cyan)),
                    Span::raw(r.clone()),
                ])
            } else {
                Line::from(vec![Span::raw("  "), Span::raw(r.clone())])
            }
        })
        .collect();
    let placeholder = app.input.is_empty() && app.modal.is_none();
    let mut lines = lines;
    if placeholder {
        lines = vec![Line::from(vec![
            Span::styled("› ", Style::default().fg(Color::Cyan)),
            Span::styled("message, /command, or shift+tab to change mode", dim()),
        ])];
    }
    let block = UiBlock::default().borders(Borders::ALL).border_style(dim());
    f.render_widget(Paragraph::new(lines).block(block), area);
    if app.modal.is_none() {
        let x = area.x + 1 + 2 + cpos.1 as u16;
        let y = area.y + 1 + (cpos.0 - first) as u16;
        f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(2)), y));
    }
}

fn render_status(app: &App, f: &mut Frame, area: Rect) {
    let mode = if app.mode_id.is_empty() {
        "default"
    } else {
        &app.mode_id
    };
    let mut left = vec![
        Span::styled(
            format!(" {mode} "),
            Style::default()
                .fg(Color::Black)
                .bg(mode_color(mode))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", app.model_label), dim()),
    ];
    if let Some(id) = &app.session_id {
        left.push(Span::styled(format!(" · {id}"), dim()));
    }
    left.push(Span::styled(format!(" · {}", app.tokens_label()), dim()));
    let right = if app.running {
        format!("{} esc to interrupt ", spinner(app))
    } else {
        "shift+tab mode · /help ".to_string()
    };
    let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(used + right.chars().count());
    left.push(Span::raw(" ".repeat(pad)));
    left.push(Span::styled(
        right,
        if app.running {
            Style::default().fg(Color::Yellow)
        } else {
            dim()
        },
    ));
    f.render_widget(Paragraph::new(Line::from(left)), area);
}

// ---- modals ---------------------------------------------------------------------

fn centered(area: Rect, w_pct: u16, h: u16) -> Rect {
    let w = (area.width * w_pct / 100).max(30).min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}

fn render_modal(m: &Modal, f: &mut Frame, area: Rect) {
    let (title, lines) = match m {
        Modal::Permission(p) => ("Permission".to_string(), permission_lines(p)),
        Modal::Ask(a) => (
            format!(" Question {}/{} ", a.page + 1, a.questions.len()),
            ask_lines(a),
        ),
        Modal::Picker(p) => (" Resume a session ".to_string(), picker_lines(p)),
        Modal::Models(p) => (" Model for this session ".to_string(), model_lines(p)),
    };
    let width = (area.width * 80 / 100).max(30).min(area.width);
    let inner_w = width.saturating_sub(2) as usize;
    let lines: Vec<Line> = lines
        .into_iter()
        .flat_map(|l| wrap_line(l, inner_w))
        .collect();
    let rect = centered(area, 80, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = UiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title)
        .title_alignment(Alignment::Left);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn sel_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn permission_lines(p: &PermissionModal) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(Span::styled(
        p.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for s in &p.summary {
        l.push(Line::from(Span::styled(format!("  {s}"), dim())));
    }
    l.push(Line::default());
    for (i, o) in p.options.iter().enumerate() {
        let sel = i == p.selected;
        l.push(Line::from(Span::styled(
            format!("{} {}. {}", if sel { "❯" } else { " " }, i + 1, o.name),
            sel_style(sel),
        )));
    }
    l.push(Line::default());
    l.push(Line::from(Span::styled(
        "↑/↓ select · enter confirm · 1-9 quick pick · esc reject",
        dim(),
    )));
    l
}

fn ask_lines(a: &AskModal) -> Vec<Line<'static>> {
    let q = &a.questions[a.page];
    let st = &a.states[a.page];
    let mut l = vec![];
    // Page tabs.
    if a.questions.len() > 1 {
        let tabs: Vec<Span> = a
            .questions
            .iter()
            .enumerate()
            .flat_map(|(i, q)| {
                let name = if q.header.is_empty() {
                    format!("Q{}", i + 1)
                } else {
                    q.header.clone()
                };
                [
                    Span::styled(
                        format!(" {name} "),
                        if i == a.page {
                            Style::default().fg(Color::Black).bg(Color::Cyan)
                        } else {
                            dim()
                        },
                    ),
                    Span::raw(" "),
                ]
            })
            .collect();
        l.push(Line::from(tabs));
        l.push(Line::default());
    }
    l.push(Line::from(Span::styled(
        q.question.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    l.push(Line::default());
    for (i, o) in q.options.iter().enumerate() {
        l.push(option_line(i, &o.label, st, q.multi_select));
        if !o.description.is_empty() {
            l.push(Line::from(Span::styled(
                format!("      {}", o.description),
                dim(),
            )));
        }
    }
    let other = q.options.len();
    let mut ol = option_line(other, "Other…", st, q.multi_select);
    if st.editing || !st.other_text.is_empty() {
        ol.spans.push(Span::styled(
            format!(": {}{}", st.other_text, if st.editing { "▏" } else { "" }),
            Style::default().fg(Color::Yellow),
        ));
    }
    l.push(ol);
    l.push(Line::default());
    let hint = if st.editing {
        "type your answer · enter done · esc discard"
    } else if q.multi_select {
        "↑/↓ move · space toggle · enter next/confirm · ←/→ page · esc cancel"
    } else {
        "↑/↓ move · space/enter select · ←/→ page · esc cancel"
    };
    l.push(Line::from(Span::styled(hint, dim())));
    l
}

fn option_line(i: usize, label: &str, st: &super::app::QState, multi: bool) -> Line<'static> {
    let on_cursor = st.cursor == i;
    let checked = st.checked.get(i).copied().unwrap_or(false);
    let mark = match (multi, checked) {
        (true, true) => "[x]",
        (true, false) => "[ ]",
        (false, true) => "(•)",
        (false, false) => "( )",
    };
    Line::from(Span::styled(
        format!(
            "{} {} {}. {}",
            if on_cursor { "❯" } else { " " },
            mark,
            i + 1,
            label
        ),
        sel_style(on_cursor),
    ))
}

fn model_lines(p: &super::app::ModelPickerModal) -> Vec<Line<'static>> {
    let mut l = vec![];
    let items =
        std::iter::once("(mode / profile default)".to_string()).chain(p.models.iter().cloned());
    for (i, m) in items.enumerate().take(14) {
        let s = i == p.selected;
        l.push(Line::from(Span::styled(
            format!("{} {m}", if s { "❯" } else { " " }),
            sel_style(s),
        )));
    }
    l.push(Line::default());
    l.push(Line::from(Span::styled(
        "↑/↓ select · enter switch · esc cancel",
        dim(),
    )));
    l
}

fn picker_lines(p: &PickerModal) -> Vec<Line<'static>> {
    let mut l = vec![];
    for (i, s) in p.sessions.iter().enumerate().take(12) {
        let sel = i == p.selected;
        l.push(Line::from(Span::styled(
            format!(
                "{} {}  {:<9} {}",
                if sel { "❯" } else { " " },
                s.updated_at.get(..16).unwrap_or(&s.updated_at),
                s.mode,
                crate::one_line(&s.first_prompt, 50)
            ),
            sel_style(sel),
        )));
    }
    l.push(Line::default());
    l.push(Line::from(Span::styled(
        "↑/↓ select · enter resume · esc cancel",
        dim(),
    )));
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{AskRequest, Question, QuestionOption};
    use ez_acp::schema::{
        ContentBlock, ContentChunk, Diff, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
        PlanEntryPriority, RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallContent,
        ToolCallUpdate, ToolCallUpdateFields,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::sync::oneshot;

    fn draw(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(app, f)).unwrap();
        let buf = t.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn wrap_and_input_layout() {
        let l = wrap_line(Line::from("aaaa bbbb cccc"), 6);
        let texts: Vec<String> = l.iter().map(|l| l.to_string()).collect();
        assert_eq!(texts, vec!["aaaa ", "bbbb ", "cccc"]);
        let (rows, c) = input_layout("ab\ncd", 4, 10);
        assert_eq!(rows, vec!["ab", "cd"]);
        assert_eq!(c, (1, 1));
        let (rows, c) = input_layout("abcd", 4, 2);
        assert_eq!(rows, vec!["ab", "cd", ""]);
        assert_eq!(c, (2, 0));
    }

    #[test]
    fn smoke_transcript_tools_plan_status() {
        let mut app = App::new("mock/mock");
        app.session_id = Some("sess-1".into());
        app.mode_id = "implement".into();
        app.begin_turn("fix the bug".into());
        app.handle_update(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::from("Looking now\n```\nlet x = 1;\n```"),
        )));
        app.handle_update(SessionUpdate::ToolCall(
            ToolCall::new("c", "Edit src/lib.rs").status(ToolCallStatus::InProgress),
        ));
        app.handle_update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "c",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![ToolCallContent::Diff(
                    Diff::new("src/lib.rs", "b\n").old_text("a\n"),
                )]),
        )));
        app.handle_update(SessionUpdate::Plan(Plan::new(vec![PlanEntry::new(
            "write tests",
            PlanEntryPriority::High,
            PlanEntryStatus::InProgress,
        )])));
        let s = draw(&app, 80, 24);
        for want in [
            "fix the bug",
            "Looking now",
            "let x = 1;",
            "Edit src/lib.rs",
            "✓",
            "+b",
            "-a",
            "write tests",
            "implement",
            "mock/mock",
            "sess-1",
            "esc to interrupt",
        ] {
            assert!(s.contains(want), "missing {want:?} in:\n{s}");
        }
    }

    #[test]
    fn thoughts_collapse_and_expand() {
        let mut app = App::new("m");
        app.handle_update(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::from("secret reasoning"),
        )));
        let s = draw(&app, 60, 12);
        assert!(s.contains("thought") && !s.contains("secret reasoning"));
        app.show_thoughts = true;
        assert!(draw(&app, 60, 12).contains("secret reasoning"));
    }

    #[test]
    fn modals_render() {
        let mut app = App::new("m");
        let (tx, _rx) = oneshot::channel();
        app.request_permission(
            RequestPermissionRequest::new(
                "s",
                ToolCallUpdate::new(
                    "c",
                    ToolCallUpdateFields::new()
                        .title("Bash: rm -rf x")
                        .raw_input(serde_json::json!({"command":"rm -rf x"})),
                ),
                vec![PermissionOption::new(
                    "a",
                    "Allow once",
                    PermissionOptionKind::AllowOnce,
                )],
            ),
            tx,
        );
        let s = draw(&app, 80, 24);
        assert!(
            s.contains("Bash: rm -rf x")
                && s.contains("Allow once")
                && s.contains("command: rm -rf x")
        );

        let mut app = App::new("m");
        let (tx, _rx) = oneshot::channel();
        app.ask_questions(
            AskRequest {
                session_id: "s".into(),
                questions: vec![Question {
                    question: "Which db?".into(),
                    header: "DB".into(),
                    options: vec![QuestionOption {
                        label: "Postgres".into(),
                        description: "relational".into(),
                    }],
                    multi_select: false,
                }],
            },
            tx,
        );
        let s = draw(&app, 80, 24);
        assert!(
            s.contains("Which db?")
                && s.contains("Postgres")
                && s.contains("relational")
                && s.contains("Other…")
        );
    }

    #[test]
    fn scrolling_sets_bounds() {
        let mut app = App::new("m");
        for i in 0..100 {
            app.notice(format!("line {i}"));
        }
        let s = draw(&app, 40, 12);
        assert!(s.contains("line 99") && !s.contains("line 10\n"));
        assert!(app.max_scroll.get() > 0);
        app.scroll_by(1000);
        let s = draw(&app, 40, 12);
        assert!(s.contains("line 0"));
    }
}
