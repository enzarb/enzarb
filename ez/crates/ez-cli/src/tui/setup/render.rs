//! Rendering for the setup wizard.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::*;

const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn sel(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn marker(selected: bool) -> &'static str {
    if selected { "❯ " } else { "  " }
}

fn err_line(w: &SetupWizard) -> Option<Line<'static>> {
    w.error.as_ref().map(|e| {
        Line::from(Span::styled(
            format!("! {e}"),
            Style::default().fg(Color::Red),
        ))
    })
}

/// Draw one text field line; returns the cursor column when `focus`.
fn field(
    label: &str,
    input: &TextInput,
    masked: bool,
    focus: bool,
) -> (Line<'static>, Option<usize>) {
    let shown = if masked {
        "•".repeat(input.value.chars().count())
    } else {
        input.value.clone()
    };
    let prefix = format!("{label}: ");
    let col = prefix.chars().count() + input.cursor_col();
    (
        Line::from(vec![
            Span::styled(prefix, dim()),
            Span::styled(shown, sel(focus)),
        ]),
        focus.then_some(col),
    )
}

pub fn render(w: &SetupWizard, f: &mut Frame) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);
    let head = vec![
        Line::from(vec![
            Span::styled("ez setup", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "  step {} of {}: {}",
                    w.step.index(),
                    TOTAL_STEPS,
                    w.step.title()
                ),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(Span::styled("─".repeat(area.width as usize), dim())),
    ];
    f.render_widget(Paragraph::new(head), chunks[0]);

    let body = chunks[1].inner(ratatui::layout::Margin::new(1, 0));
    let (lines, cursor): (Vec<Line>, Option<(usize, usize)>) = match w.step {
        Step::ApiKey => auth_lines(w),
        Step::Endpoint => endpoint_lines(w),
        Step::Check => (check_lines(w), None),
        Step::ProfileName => profile_lines(w),
        Step::Models => models_lines(w, body.height as usize),
        Step::Modes => (modes_lines(w, body.height as usize), None),
        Step::Review => (review_lines(w, body.height as usize), None),
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    if let Some((row, col)) = cursor {
        f.set_cursor_position(Position::new(
            body.x + col as u16,
            body.y + (row as u16).min(body.height.saturating_sub(1)),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint(w), dim()))),
        chunks[2],
    );

    if let (
        Step::Modes,
        ModesPhase::Assign {
            row,
            chooser: Some(ch),
        },
    ) = (w.step, &w.modes_phase)
    {
        chooser_popup(w, *row, ch, f, area);
    }
}

fn hint(w: &SetupWizard) -> String {
    let base = "ctrl+c abort · esc back";
    match w.step {
        Step::Endpoint => format!("enter check · {base}"),
        Step::ProfileName => format!("enter continue · {base}"),
        Step::ApiKey => format!(
            "↑/↓ option · type/paste the key · ←/→ choose saved key · enter continue · {base}"
        ),
        Step::Check => match &w.check {
            CheckPhase::Checking => base.to_string(),
            CheckPhase::Detected { .. } => {
                format!("enter continue · o try the other API · r retry · {base}")
            }
            CheckPhase::Failed { .. } => {
                format!("r retry · o force the other API · m type model ids by hand · {base}")
            }
        },
        Step::Models => match &w.models {
            ModelsPhase::List(_) => format!(
                "space toggle · a all · type to filter ( / for names starting with a) · enter confirm · {base}"
            ),
            _ => base.to_string(),
        },
        Step::Modes => format!("↑/↓ mode · enter change · c continue · {base}"),
        Step::Review => format!("enter save · d toggle default · ↑/↓ scroll · {base}"),
    }
}

fn endpoint_lines(w: &SetupWizard) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let (f, c) = field("Base URL", &w.endpoint, false, true);
    let mut l = vec![
        Line::from(
            "Where is the API? The URL is required (Anthropic or any OpenAI-compatible server).",
        ),
        Line::default(),
        f,
    ];
    l.extend(err_line(w));
    if w.error.is_none()
        && !w.endpoint.trimmed().is_empty()
        && let Err(e) = setup::validate_endpoint(w.endpoint.trimmed())
    {
        l.push(Line::from(Span::styled(
            format!("! {e}"),
            Style::default().fg(Color::Yellow),
        )));
    }
    (l, c.map(|c| (2, c)))
}

fn check_lines(w: &SetupWizard) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(format!("Checking {} …", w.endpoint.trimmed()))];
    l.push(Line::default());
    match &w.check {
        CheckPhase::Checking => l.push(Line::from(format!(
            "{} probing the Anthropic and OpenAI-compatible APIs",
            spin(w)
        ))),
        CheckPhase::Detected { message } => {
            l.push(Line::from(Span::styled(
                message.clone(),
                Style::default().fg(Color::Green),
            )));
            if w.forced.is_some() {
                l.push(Line::from(Span::styled("(flavor chosen manually)", dim())));
            }
        }
        CheckPhase::Failed { message } => {
            l.push(Line::from(Span::styled(
                message.clone(),
                Style::default().fg(Color::Red),
            )));
        }
    }
    l
}

fn auth_lines(w: &SetupWizard) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let mut l = vec![
        Line::from(
            "Enter your API key. It is saved to your per-user credentials file when you finish.",
        ),
        Line::default(),
    ];
    let on = |i: usize| w.auth_cursor == i;
    let secret = "•".repeat(w.new_key_secret.value.chars().count());
    let head = format!("{}paste an API key: ", marker(on(0)));
    let col = head.chars().count() + w.new_key_secret.cursor_col();
    l.push(Line::from(vec![
        Span::styled(head, sel(on(0))),
        Span::styled(secret, sel(on(0))),
    ]));
    let cursor = on(0).then_some((l.len() - 1, col));
    let existing = match w.ctx.keys.get(w.existing_idx) {
        Some(k) => format!("◂ {} ({}) ▸", k.name, k.masked),
        None => "(no saved keys yet)".into(),
    };
    l.push(Line::from(Span::styled(
        format!("{}use a saved key: {existing}", marker(on(1))),
        sel(on(1)),
    )));
    l.push(Line::from(Span::styled(
        format!(
            "{}no key (local servers such as Ollama, LM Studio, vLLM)",
            marker(on(2))
        ),
        sel(on(2)),
    )));
    l.push(Line::default());
    l.push(Line::from(Span::styled(
        "Keys live only in credentials.toml (mode 0600) in your user config directory; never in the project or config.toml.",
        dim(),
    )));
    l.extend(err_line(w));
    (l, cursor)
}

fn profile_lines(w: &SetupWizard) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let (f, c) = field("Profile name", &w.profile_name, false, !w.confirm_overwrite);
    let mut l = vec![
        Line::from("Name this profile (used in config.toml and in modes.toml):"),
        Line::default(),
        f,
    ];
    if w.confirm_overwrite {
        l.push(Line::default());
        l.push(Line::from(Span::styled(
            format!(
                "Profile `{}` already exists. Overwrite its provider, model, models and key? (y/n)",
                w.profile_name.trimmed()
            ),
            Style::default().fg(Color::Yellow),
        )));
    }
    l.extend(err_line(w));
    (
        l,
        if w.confirm_overwrite {
            None
        } else {
            c.map(|c| (2, c))
        },
    )
}

fn spin(w: &SetupWizard) -> &'static str {
    SPINNER[(w.tick / 2) as usize % SPINNER.len()]
}

fn window(cursor: usize, len: usize, height: usize) -> std::ops::Range<usize> {
    let h = height.max(1);
    let start = cursor.saturating_sub(h / 2).min(len.saturating_sub(h));
    start..(start + h).min(len)
}

fn models_lines(w: &SetupWizard, height: usize) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let mut l = vec![];
    match &w.models {
        ModelsPhase::Manual { input, err } => {
            l.push(Line::from(
                "Enter the model ids you want available, comma-separated:",
            ));
            l.push(Line::default());
            let (f, c) = field("Models", input, false, true);
            l.push(f);
            l.extend(err.as_ref().map(|e| {
                Line::from(Span::styled(
                    format!("! {e}"),
                    Style::default().fg(Color::Red),
                ))
            }));
            return (l, c.map(|c| (2, c)));
        }
        ModelsPhase::List(st) => {
            let vis = st.visible();
            l.push(Line::from(format!(
                "Which models should be available? ({} selected, {} shown of {})",
                w.selected.len(),
                vis.len(),
                st.items.len()
            )));
            let (f, c) = field(
                "Filter",
                &TextInput::new(st.filter.clone()),
                false,
                st.filter_focus,
            );
            l.push(f);
            let cursor = c.map(|c| (1, c));
            let avail = height.saturating_sub(4).max(1);
            for pos in window(st.cursor, vis.len(), avail) {
                let m = &st.items[vis[pos]];
                let on = pos == st.cursor;
                let check = if w.selected.contains(&m.id) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let mut spans = vec![Span::styled(
                    format!("{}{check} {}", marker(on), m.id),
                    sel(on),
                )];
                if let Some(d) = &m.display_name {
                    spans.push(Span::styled(format!("  {d}"), dim()));
                }
                l.push(Line::from(spans));
            }
            if vis.is_empty() {
                l.push(Line::from(Span::styled(
                    "(no models match the filter)",
                    dim(),
                )));
            }
            l.extend(err_line(w));
            return (l, cursor);
        }
        ModelsPhase::Default { choices, cursor } => {
            l.push(Line::from(
                "Which one should be this profile's default model?",
            ));
            l.push(Line::default());
            for pos in window(*cursor, choices.len(), height.saturating_sub(3)) {
                l.push(Line::from(Span::styled(
                    format!("{}{}", marker(pos == *cursor), choices[pos]),
                    sel(pos == *cursor),
                )));
            }
        }
    }
    (l, None)
}

fn choice_label(c: &Option<ModelChoice>) -> String {
    match c {
        Some((p, m)) => format!("{p}/{m}"),
        None => "(profile default)".into(),
    }
}

fn modes_lines(w: &SetupWizard, height: usize) -> Vec<Line<'static>> {
    let mut l = vec![];
    match &w.modes_phase {
        ModesPhase::NeedInit { cursor, error } => {
            l.push(Line::from("This project has no .ez/modes.toml yet."));
            l.push(Line::from(
                "Per-mode models are saved there so your team shares them.",
            ));
            l.push(Line::default());
            for (i, t) in [
                "run `ez init` now (generate modes from templates and mise tasks)",
                "skip per-mode models",
            ]
            .iter()
            .enumerate()
            {
                l.push(Line::from(Span::styled(
                    format!("{}{t}", marker(i == *cursor)),
                    sel(i == *cursor),
                )));
            }
            if let Some(e) = error {
                l.push(Line::from(Span::styled(
                    format!("! ez init failed: {e}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        ModesPhase::Initing => l.push(Line::from(format!("{} generating modes.toml …", spin(w)))),
        ModesPhase::Broken => {
            if let ModesState::Broken(e) = &w.ctx.modes {
                l.push(Line::from(Span::styled(
                    format!("Cannot read .ez/modes.toml: {e}"),
                    Style::default().fg(Color::Red),
                )));
            }
            l.push(Line::default());
            l.push(Line::from(
                "Press enter to continue without per-mode models.",
            ));
        }
        ModesPhase::Assign { row, .. } => {
            let Some(set) = w.mode_set() else { return l };
            l.push(Line::from(
                "Choose a model for each mode (enter to change); the rest use the profile default.",
            ));
            l.push(Line::default());
            let width = set.modes.iter().map(|(id, _)| id.len()).max().unwrap_or(4);
            let avail = height.saturating_sub(4).max(1);
            let total = set.modes.len() + 1;
            for pos in window(*row, total, avail) {
                let on = pos == *row;
                if pos == set.modes.len() {
                    l.push(Line::from(Span::styled(
                        format!("{}continue →", marker(on)),
                        sel(on),
                    )));
                    continue;
                }
                let (id, _) = &set.modes[pos];
                let a = w.assignments.get(pos).cloned().flatten();
                let style = if a.is_some() { sel(on) } else { dim() };
                l.push(Line::from(vec![
                    Span::styled(format!("{}{id:<width$}  ", marker(on)), sel(on)),
                    Span::styled(choice_label(&a), if on { sel(true) } else { style }),
                ]));
            }
        }
    }
    l
}

fn chooser_popup(w: &SetupWizard, row: usize, ch: &Chooser, f: &mut Frame, area: Rect) {
    let items = w.chooser_items(&ch.filter);
    let h = (items.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let width = area.width.saturating_sub(6).clamp(20, 80);
    let r = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        width,
        h,
    );
    f.render_widget(Clear, r);
    let mode = w
        .mode_set()
        .and_then(|s| s.modes.get(row))
        .map(|(id, _)| id.clone())
        .unwrap_or_default();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" model for `{mode}` "));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![Line::from(vec![
        Span::styled("filter: ", dim()),
        Span::raw(ch.filter.clone()),
    ])];
    for pos in window(
        ch.cursor,
        items.len(),
        inner.height.saturating_sub(1) as usize,
    ) {
        lines.push(Line::from(Span::styled(
            format!("{}{}", marker(pos == ch.cursor), choice_label(&items[pos])),
            sel(pos == ch.cursor),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn review_lines(w: &SetupWizard, height: usize) -> Vec<Line<'static>> {
    let mut l = vec![];
    let p = w.plan();
    l.push(Line::from(vec![
        Span::styled("Profile ", dim()),
        Span::styled(
            p.profile_name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} · default model {} · {} model(s) · {}",
                p.profile.provider.as_str(),
                p.profile.model,
                p.profile.models.len(),
                if w.set_default {
                    "will be the default profile"
                } else {
                    "not the default profile"
                }
            ),
            dim(),
        ),
    ]));
    if let Some(set) = w.mode_set()
        && !w.modes_skipped
    {
        let table: Vec<String> = set
            .modes
            .iter()
            .enumerate()
            .map(|(i, (id, _))| {
                format!(
                    "{id}={}",
                    choice_label(&w.assignments.get(i).cloned().flatten())
                )
            })
            .collect();
        l.push(Line::from(Span::styled(table.join("  "), dim())));
    }
    l.push(Line::default());
    let avail = height.saturating_sub(l.len() + 2).max(1);
    let diff: Vec<&str> = w.preview.lines().collect();
    let start = w.preview_scroll.min(diff.len().saturating_sub(avail));
    for line in diff.iter().skip(start).take(avail) {
        let style = if line.starts_with('+') && !line.starts_with("+++") {
            Style::default().fg(Color::Green)
        } else if line.starts_with('-') && !line.starts_with("---") {
            Style::default().fg(Color::Red)
        } else if line.starts_with('#') || line.starts_with("key `") || line.starts_with("API") {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        l.push(Line::from(Span::styled((*line).to_string(), style)));
    }
    if let Some(e) = &w.apply_error {
        l.push(Line::from(Span::styled(
            format!("! could not save: {e}"),
            Style::default().fg(Color::Red),
        )));
    }
    l
}
