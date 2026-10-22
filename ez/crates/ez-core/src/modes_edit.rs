//! Writes a [`ModesDoc`] back to `modes.toml` text, changing only what was edited.
//!
//! The file is split into *sections* (one per explicit `[table]` / `[[array]]` header, found with
//! `toml_edit` spans so multi-line strings cannot confuse it) and sections are grouped per mode.
//! Everything between sections (blank lines, comments directly above a header) stays attached to
//! the section below it. Then:
//!
//! * modes are re-ordered, removed, renamed and added at the text level (a renamed mode only has
//!   its headers rewritten, so its comments stay);
//! * edits inside a mode go through `toml_edit` on that section's body, touching only keys whose
//!   value changed, keeping the value's decor (trailing comments) and the comments above a key;
//! * hooks are `[[modes.<id>.hooks.<event>]]` sections: unchanged entries are reused verbatim,
//!   changed ones are edited in place, new ones are generated;
//! * a mode whose layout is not the one `ez init` writes (inline tables, dotted keys, ...) is
//!   written out fresh instead (reported in [`Rendered::rewritten`]); modes that did not change are
//!   never touched, whatever their layout.
//!
//! The result is re-parsed and compared with the requested document; on a mismatch the changed
//! modes are rewritten fresh, and only then does it give up.

use std::collections::BTreeSet;
use std::ops::Range;

use anyhow::{Context, Result, bail};
use toml_edit::{Array, Document, DocumentMut, Item, Table, Value};

use crate::modes_api::{
    DEFAULT_MAX_FIX_TURNS, DEFAULT_TIMEOUT_SECS, HOOK_EVENTS, HookDto, ModeDto, ModesDoc, parse_doc,
};

pub struct Rendered {
    pub text: String,
    pub rewritten: Vec<String>,
}

pub fn render(old: Option<&str>, doc: &ModesDoc) -> Result<Rendered> {
    let doc = doc.clone().normalized();
    for force in [false, true] {
        let r = render_once(old.unwrap_or(""), &doc, force)?;
        if let Ok(p) = parse_doc(&r.text)
            && equivalent(&p, &doc)
        {
            return Ok(r);
        }
    }
    bail!("could not write modes.toml faithfully (internal error); edit the file by hand")
}

fn equivalent(parsed: &ModesDoc, want: &ModesDoc) -> bool {
    parsed.default_mode == want.default_mode
        && parsed.modes.len() == want.modes.len()
        && parsed
            .modes
            .iter()
            .zip(&want.modes)
            .all(|(a, b)| a.same_content(b))
}

// ----- layout -----

struct Sec {
    path: Vec<String>,
    array: bool,
    /// Blank lines and comments directly above the header.
    lead: String,
    header: String,
    /// Rest of the header line, then the body (up to the next section's lead).
    rest: String,
    new: bool,
    was_last: bool,
}

impl Sec {
    fn text(&self) -> String {
        format!("{}{}{}", self.lead, self.header, self.rest)
    }

    fn set_tail(&mut self, n: usize) {
        let t = self.rest.trim_end_matches('\n').len();
        self.rest.truncate(t);
        self.rest.push_str(&"\n".repeat(n));
    }
}

struct Group {
    id: String,
    secs: Vec<Sec>,
    canonical: bool,
}

impl Group {
    fn raw(&self) -> String {
        self.secs.iter().map(Sec::text).collect()
    }
}

struct Layout {
    preamble: String,
    groups: Vec<Group>,
    trailer: String,
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// Start of the comment lines directly above the header at `header_start` (not before `lower`).
fn lead_start(text: &str, header_start: usize, lower: usize) -> usize {
    let mut start = line_start(text, header_start).max(lower);
    while start > lower {
        let prev_start = line_start(text, start - 1);
        if prev_start < lower || !text[prev_start..start - 1].trim_start().starts_with('#') {
            break;
        }
        start = prev_start;
    }
    start
}

type Found = (Range<usize>, Vec<String>, bool);

fn collect(t: &Table, path: &mut Vec<String>, out: &mut Vec<Found>) {
    for (k, v) in t.iter() {
        path.push(k.to_string());
        match v {
            Item::Table(t2) => {
                if let (Some(_), Some(span)) = (t2.position(), t2.span()) {
                    out.push((span, path.clone(), false));
                }
                collect(t2, path, out);
            }
            Item::ArrayOfTables(a) => {
                for t2 in a.iter() {
                    if let Some(span) = t2.span() {
                        out.push((span, path.clone(), true));
                    }
                    collect(t2, path, out);
                }
            }
            _ => {}
        }
        path.pop();
    }
}

fn split(text: &str) -> Result<Layout> {
    let parsed = Document::parse(text).context("parsing modes.toml")?;
    let mut found = Vec::new();
    collect(parsed.as_table(), &mut vec![], &mut found);
    found.sort_by_key(|f| f.0.start);

    let mut secs: Vec<Sec> = Vec::new();
    let mut lower = 0;
    let mut starts = Vec::new();
    for (span, path, array) in &found {
        let ls = lead_start(text, span.start, lower);
        starts.push(ls);
        secs.push(Sec {
            path: path.clone(),
            array: *array,
            lead: text[ls..span.start].to_string(),
            header: text[span.clone()].to_string(),
            rest: String::new(),
            new: false,
            was_last: false,
        });
        lower = line_end(text, span.end);
    }
    for i in 0..secs.len() {
        let end = starts.get(i + 1).copied().unwrap_or(text.len());
        secs[i].rest = text[found[i].0.end..end].to_string();
    }

    let is_mode = |s: &Sec| s.path.len() >= 2 && s.path[0] == "modes";
    let first = secs.iter().position(is_mode);
    let preamble_end = first.map_or(text.len(), |i| starts[i]);
    let mut groups: Vec<Group> = Vec::new();
    let mut foreign = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for s in secs.into_iter().skip(first.unwrap_or(usize::MAX)) {
        if is_mode(&s) {
            if groups.last().map(|g| g.id.as_str()) != Some(s.path[1].as_str()) {
                if !seen.insert(s.path[1].clone()) {
                    bail!(
                        "the sections of mode `{}` are not contiguous in modes.toml; edit the file by hand",
                        s.path[1]
                    );
                }
                groups.push(Group {
                    id: s.path[1].clone(),
                    secs: vec![],
                    canonical: true,
                });
            }
        } else {
            foreign.insert(groups.len() - 1);
        }
        groups.last_mut().expect("group").secs.push(s);
    }
    for (gi, g) in groups.iter_mut().enumerate() {
        g.canonical = !foreign.contains(&gi) && is_canonical(g, &parsed);
    }

    // The trailing comment block at the end of the file stays at the end of the file.
    let mut trailer = String::new();
    if let Some(sec) = groups.last_mut().and_then(|g| g.secs.last_mut()) {
        let (body, tr) = split_trailer(&sec.rest);
        trailer = tr;
        sec.rest = body;
    }
    Ok(Layout {
        preamble: text[..preamble_end].to_string(),
        groups,
        trailer,
    })
}

/// Splits `rest` into (body, trailer): the trailer starts at the first blank line of the blank /
/// comment-only suffix.
fn split_trailer(rest: &str) -> (String, String) {
    let lines: Vec<&str> = rest.split_inclusive('\n').collect();
    let mut i = lines.len();
    while i > 1 && {
        let t = lines[i - 1].trim();
        t.is_empty() || t.starts_with('#')
    } {
        i -= 1;
    }
    match (i..lines.len()).find(|&j| lines[j].trim().is_empty()) {
        Some(j) => (lines[..j].concat(), lines[j..].concat()),
        None => (rest.to_string(), String::new()),
    }
}

fn is_canonical(g: &Group, parsed: &Document<&str>) -> bool {
    let id = &g.id;
    let first = &g.secs[0];
    if first.path.len() != 2 || first.array {
        return false;
    }
    let mut perms = 0;
    let mut hook_secs = 0;
    for s in &g.secs[1..] {
        match (s.path.as_slice(), s.array) {
            ([_, _, p], false) if p == "permissions" => perms += 1,
            ([_, _, h, e], true) if h == "hooks" && HOOK_EVENTS.iter().any(|x| x.as_str() == e) => {
                hook_secs += 1
            }
            _ => return false,
        }
    }
    if perms > 1 {
        return false;
    }
    let Some(t) = parsed
        .get("modes")
        .and_then(|m| m.get(id))
        .and_then(Item::as_table)
    else {
        return false;
    };
    let mut hooks_seen = 0;
    for (k, v) in t.iter() {
        match k {
            "permissions" => {
                if !matches!(v, Item::Table(p) if !p.is_dotted() && p.position().is_some()) {
                    return false;
                }
            }
            "hooks" => {
                let Item::Table(h) = v else { return false };
                for (_, a) in h.iter() {
                    let Item::ArrayOfTables(a) = a else {
                        return false;
                    };
                    hooks_seen += a.len();
                }
            }
            _ => {
                if !v.is_value() {
                    return false;
                }
            }
        }
    }
    hooks_seen == hook_secs
}

// ----- values -----

fn basic_ml(s: &str) -> String {
    let mut out = String::from("\"\"\"\n");
    let mut run = 0;
    for c in s.chars() {
        if c == '"' {
            run += 1;
            if run >= 3 {
                out.push_str("\\\"");
                run = 0;
                continue;
            }
            out.push('"');
            continue;
        }
        run = 0;
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push_str("\"\"\"");
    out
}

fn str_val(s: &str, multiline: bool) -> Value {
    if multiline && s.contains('\n') {
        basic_ml(s)
            .parse::<Value>()
            .unwrap_or_else(|_| Value::from(s))
    } else {
        Value::from(s)
    }
}

fn arr_val(v: &[String]) -> Value {
    let mut a = Array::new();
    for x in v {
        a.push(x.as_str());
    }
    Value::Array(a)
}

fn key_repr(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        serde_json::to_string(k).expect("string")
    }
}

fn header_for(path: &[String], array: bool) -> String {
    let p: Vec<String> = path.iter().map(|k| key_repr(k)).collect();
    if array {
        format!("[[{}]]", p.join("."))
    } else {
        format!("[{}]", p.join("."))
    }
}

type Edit = (String, Option<Value>);

fn mode_edits(old: &ModeDto, new: &ModeDto) -> Vec<Edit> {
    let mut e: Vec<Edit> = Vec::new();
    let opt = |o: &Option<String>| o.as_deref().map(Value::from);
    let list = |l: &[String]| (!l.is_empty()).then(|| arr_val(l));
    if old.name != new.name {
        e.push(("name".to_string(), opt(&new.name)));
    }
    if old.description != new.description {
        e.push((
            "description".to_string(),
            (!new.description.is_empty()).then(|| str_val(&new.description, false)),
        ));
    }
    if old.prompt != new.prompt {
        e.push((
            "prompt".to_string(),
            (!new.prompt.is_empty()).then(|| str_val(&new.prompt, true)),
        ));
    }
    if old.tools != new.tools {
        e.push(("tools".to_string(), new.tools.as_deref().map(arr_val)));
    }
    if old.mise_tasks != new.mise_tasks {
        e.push(("mise_tasks".to_string(), list(&new.mise_tasks)));
    }
    if old.mcp_tools != new.mcp_tools {
        e.push(("mcp_tools".to_string(), list(&new.mcp_tools)));
    }
    if old.next != new.next {
        e.push(("next".to_string(), list(&new.next)));
    }
    if old.auto_advance_to != new.auto_advance_to {
        e.push(("auto_advance_to".to_string(), list(&new.auto_advance_to)));
    }
    if old.profile != new.profile {
        e.push(("profile".to_string(), opt(&new.profile)));
    }
    if old.model != new.model {
        e.push(("model".to_string(), opt(&new.model)));
    }
    e
}

fn hook_edits(old: &HookDto, new: &HookDto) -> Vec<Edit> {
    let mut e: Vec<Edit> = Vec::new();
    let opt = |o: &Option<String>| o.as_deref().map(Value::from);
    if old.run != new.run {
        e.push(("run".to_string(), Some(Value::from(new.run.as_str()))));
    }
    if old.glob != new.glob {
        e.push(("glob".to_string(), opt(&new.glob)));
    }
    if old.matcher != new.matcher {
        e.push(("matcher".to_string(), opt(&new.matcher)));
    }
    if old.on_failure != new.on_failure {
        e.push((
            "on_failure".to_string(),
            (new.on_failure != "ignore").then(|| Value::from(new.on_failure.as_str())),
        ));
    }
    if old.max_fix_turns != new.max_fix_turns {
        e.push((
            "max_fix_turns".to_string(),
            (new.max_fix_turns != DEFAULT_MAX_FIX_TURNS)
                .then(|| Value::from(new.max_fix_turns as i64)),
        ));
    }
    if old.timeout_secs != new.timeout_secs {
        e.push((
            "timeout_secs".to_string(),
            (new.timeout_secs != DEFAULT_TIMEOUT_SECS)
                .then(|| Value::from(new.timeout_secs as i64)),
        ));
    }
    e
}

/// Set (or, with `None`, remove) `key`. A removed key's leading comments move to the next key,
/// or are returned when it was the last one.
fn set_key(t: &mut Table, key: &str, v: Option<Value>) -> Option<String> {
    match v {
        Some(mut nv) => {
            if let Some(item) = t.get_mut(key) {
                if let Some(old) = item.as_value() {
                    *nv.decor_mut() = old.decor().clone();
                }
                *item = Item::Value(nv);
            } else {
                t.insert(key, Item::Value(nv));
            }
            None
        }
        None => {
            let keys: Vec<String> = t.iter().map(|(k, _)| k.to_string()).collect();
            let idx = keys.iter().position(|k| k == key)?;
            let prefix = t
                .key(key)
                .and_then(|k| k.leaf_decor().prefix())
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            t.remove(key);
            // `remove` swaps the last entry into the hole: restore the file order.
            t.sort_values_by(|a, _, b, _| {
                let at = |k: &toml_edit::Key| keys.iter().position(|x| x == k.get());
                at(a).cmp(&at(b))
            });
            if !prefix.contains('#') {
                return None;
            }
            match keys.get(idx + 1) {
                Some(next) => {
                    if let Some(mut k) = t.key_mut(next) {
                        let cur = k
                            .leaf_decor()
                            .prefix()
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        k.leaf_decor_mut().set_prefix(format!("{prefix}{cur}"));
                    }
                    None
                }
                None => Some(prefix),
            }
        }
    }
}

fn edit_body(rest: &str, edits: Vec<Edit>) -> Result<String> {
    // The rest of the header line belongs to the header, not to the first key.
    let split = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let (head, body) = rest.split_at(split);
    let mut d: DocumentMut = body.parse().context("parsing a mode section")?;
    let mut orphans = String::new();
    for (k, v) in edits {
        if let Some(o) = set_key(d.as_table_mut(), &k, v) {
            orphans.push_str(&o);
        }
    }
    if !orphans.is_empty() {
        let tr = d.trailing().as_str().unwrap_or("").to_string();
        d.set_trailing(format!("{orphans}{tr}"));
    }
    Ok(format!("{head}{d}"))
}

fn perm_edits(old: &ModeDto, new: &ModeDto) -> Vec<Edit> {
    let mut e = Vec::new();
    for k in old
        .permissions
        .keys()
        .filter(|k| !new.permissions.contains_key(*k))
    {
        e.push((k.clone(), None));
    }
    for (k, v) in &new.permissions {
        if old.permissions.get(k) != Some(v) {
            e.push((k.clone(), Some(Value::from(v.as_str()))));
        }
    }
    e
}

// ----- fresh text -----

fn kv(out: &mut String, k: &str, v: &Value) {
    out.push_str(&format!("{k} = {}\n", v.to_string().trim()));
}

fn fresh_main(m: &ModeDto) -> String {
    let mut s = format!("[modes.{}]\n", key_repr(&m.id));
    for (k, v) in mode_edits(&ModeDto::default(), m) {
        if let Some(v) = v {
            kv(&mut s, &k, &v);
        }
    }
    s
}

fn fresh_perms(id: &str, m: &ModeDto) -> String {
    let mut s = format!("[modes.{}.permissions]\n", key_repr(id));
    for (k, v) in &m.permissions {
        s.push_str(&format!("{} = {}\n", key_repr(k), Value::from(v.as_str())));
    }
    s
}

fn fresh_hook(id: &str, ev: &str, h: &HookDto) -> String {
    let mut s = format!("[[modes.{}.hooks.{ev}]]\n", key_repr(id));
    let blank = HookDto {
        run: String::new(),
        glob: None,
        matcher: None,
        on_failure: "ignore".into(),
        max_fix_turns: DEFAULT_MAX_FIX_TURNS,
        timeout_secs: DEFAULT_TIMEOUT_SECS,
    };
    for (k, v) in hook_edits(&blank, h) {
        if let Some(v) = v {
            kv(&mut s, &k, &v);
        }
    }
    s
}

fn fresh_group(m: &ModeDto) -> String {
    let mut parts = vec![fresh_main(m)];
    if !m.permissions.is_empty() {
        parts.push(fresh_perms(&m.id, m));
    }
    for ev in HOOK_EVENTS {
        for h in m.hooks.get(ev.as_str()).into_iter().flatten() {
            parts.push(fresh_hook(&m.id, ev.as_str(), h));
        }
    }
    parts.join("\n")
}

// ----- editing a group -----

fn event_rank(ev: &str) -> usize {
    HOOK_EVENTS
        .iter()
        .position(|e| e.as_str() == ev)
        .unwrap_or(usize::MAX)
}

fn new_sec(path: Vec<String>, array: bool, text: &str) -> Sec {
    let first = text.find('\n').unwrap_or(text.len());
    Sec {
        header: text[..first].to_string(),
        rest: text[first..].to_string(),
        path,
        array,
        lead: String::new(),
        new: true,
        was_last: false,
    }
}

fn edit_group(g: Group, old: &ModeDto, new: &ModeDto) -> Result<String> {
    let id = new.id.clone();
    let tail_k = g
        .secs
        .last()
        .map_or(1, |s| s.rest.len() - s.rest.trim_end_matches('\n').len());
    let last_ix = g.secs.len() - 1;
    let mut secs: Vec<Sec> = g.secs;
    for (i, s) in secs.iter_mut().enumerate() {
        s.was_last = i == last_ix;
    }

    // main section
    let edits = mode_edits(old, new);
    if !edits.is_empty() {
        secs[0].rest = edit_body(&secs[0].rest, edits)?;
    }

    // permissions
    let perms_ix = secs.iter().position(|s| !s.array && s.path.len() == 3);
    let pe = perm_edits(old, new);
    if !pe.is_empty() {
        match (perms_ix, new.permissions.is_empty()) {
            (Some(i), true) => {
                secs.remove(i);
            }
            (Some(i), false) => {
                secs[i].rest = edit_body(&secs[i].rest, pe)?;
            }
            (None, _) => {
                let text = fresh_perms(&id, new);
                secs.insert(
                    1,
                    new_sec(
                        vec!["modes".into(), id.clone(), "permissions".into()],
                        false,
                        &text,
                    ),
                );
            }
        }
    }

    // hooks
    for ev in HOOK_EVENTS {
        let evs = ev.as_str();
        let old_list: &[HookDto] = old.hooks.get(evs).map_or(&[], Vec::as_slice);
        let new_list: &[HookDto] = new.hooks.get(evs).map_or(&[], Vec::as_slice);
        if old_list == new_list {
            continue;
        }
        let ixs: Vec<usize> = secs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.array && s.path.get(3).map(String::as_str) == Some(evs))
            .map(|(i, _)| i)
            .collect();
        let insert_at = ixs.first().copied().unwrap_or_else(|| {
            secs.iter()
                .position(|s| {
                    s.array
                        && s.path
                            .get(3)
                            .is_some_and(|e| event_rank(e) > event_rank(evs))
                })
                .unwrap_or(secs.len())
        });
        let mut olds: Vec<Option<Sec>> = Vec::new();
        for &i in ixs.iter().rev() {
            olds.push(Some(secs.remove(i)));
        }
        olds.reverse();
        let mut used = vec![false; olds.len()];
        let path = vec![
            "modes".to_string(),
            id.clone(),
            "hooks".into(),
            evs.to_string(),
        ];
        let mut built = Vec::new();
        for (j, h) in new_list.iter().enumerate() {
            let exact = old_list
                .iter()
                .enumerate()
                .position(|(k, o)| !used[k] && o == h);
            let pick = exact.or_else(|| (j < old_list.len() && !used[j]).then_some(j));
            let sec = match pick {
                Some(k) => {
                    used[k] = true;
                    let mut s = olds[k].take().expect("unused");
                    if old_list[k] != *h {
                        s.rest = edit_body(&s.rest, hook_edits(&old_list[k], h))?;
                    }
                    s
                }
                None => new_sec(path.clone(), true, &fresh_hook(&id, evs, h)),
            };
            built.push(sec);
        }
        for (off, s) in built.into_iter().enumerate() {
            secs.insert(insert_at + off, s);
        }
    }

    // rename
    if old_id_of(&secs) != id {
        for s in &mut secs {
            if s.path.len() >= 2 {
                s.path[1] = id.clone();
                s.header = header_for(&s.path, s.array);
            }
        }
    }

    // spacing
    let n = secs.len();
    for (i, s) in secs.iter_mut().enumerate() {
        if i + 1 < n {
            if s.new || s.was_last {
                s.set_tail(2);
            }
        } else if s.new || !s.was_last {
            s.set_tail(tail_k.max(1));
        }
    }
    Ok(secs.iter().map(Sec::text).collect())
}

fn old_id_of(secs: &[Sec]) -> String {
    secs.first().map(|s| s.path[1].clone()).unwrap_or_default()
}

// ----- whole file -----

fn trim_nl(s: &str) -> &str {
    s.trim_end_matches('\n')
}

fn render_once(old_raw: &str, doc: &ModesDoc, force: bool) -> Result<Rendered> {
    let crlf = old_raw.contains("\r\n");
    let old_text = old_raw.replace("\r\n", "\n");
    let fresh_file = old_text.trim().is_empty();
    let (layout, old_doc) = if fresh_file {
        (
            Layout {
                preamble: String::new(),
                groups: vec![],
                trailer: String::new(),
            },
            ModesDoc::default(),
        )
    } else {
        (split(&old_text)?, parse_doc(&old_text)?)
    };

    // which on-disk mode each requested mode continues
    let mut claim: Vec<Option<usize>> = vec![None; doc.modes.len()];
    let mut taken = BTreeSet::new();
    let gix = |id: &str| layout.groups.iter().position(|g| g.id == id);
    for (i, m) in doc.modes.iter().enumerate() {
        if let Some(gi) = m.original_id.as_deref().and_then(gix)
            && taken.insert(gi)
        {
            claim[i] = Some(gi);
        }
    }
    for (i, m) in doc.modes.iter().enumerate() {
        if claim[i].is_none()
            && m.original_id.is_none()
            && let Some(gi) = gix(&m.id)
            && taken.insert(gi)
        {
            claim[i] = Some(gi);
        }
    }

    let mut groups: Vec<Option<Group>> = layout.groups.into_iter().map(Some).collect();
    let n_old = groups.len();
    let mut rewritten = Vec::new();
    let mut out: Vec<(String, Option<usize>)> = Vec::new();
    for (i, m) in doc.modes.iter().enumerate() {
        let Some(gi) = claim[i] else {
            out.push((fresh_group(m), None));
            continue;
        };
        let g = groups[gi].take().expect("claimed once");
        let old_dto = old_doc.modes.iter().find(|o| o.id == g.id).expect("parsed");
        if old_dto.same_content(m) && g.id == m.id {
            out.push((g.raw(), Some(gi)));
        } else if g.canonical && !force {
            let t = edit_group(g, old_dto, m)?;
            out.push((t, Some(gi)));
        } else {
            rewritten.push(m.id.clone());
            out.push((fresh_group(m), Some(gi)));
        }
    }

    // default_mode
    let mut preamble = layout.preamble;
    let first = doc.modes.first().map(|m| m.id.as_str());
    let mut pd: DocumentMut = preamble.parse().context("parsing modes.toml")?;
    let mut touched = false;
    match pd.get_mut("default_mode") {
        Some(item) => {
            if item.as_str() != Some(doc.default_mode.as_str()) {
                let mut nv = Value::from(doc.default_mode.as_str());
                if let Some(o) = item.as_value() {
                    *nv.decor_mut() = o.decor().clone();
                }
                *item = Item::Value(nv);
                touched = true;
            }
        }
        None => {
            if fresh_file || Some(doc.default_mode.as_str()) != first {
                pd.insert("default_mode", toml_edit::value(doc.default_mode.as_str()));
                touched = true;
            }
        }
    }
    if touched {
        let was_empty = preamble.trim().is_empty();
        preamble = pd.to_string();
        if was_empty && !preamble.ends_with("\n\n") {
            preamble.push('\n');
        }
    }
    if !preamble.is_empty() && !preamble.ends_with('\n') {
        preamble.push('\n');
    }

    let mut text = preamble;
    let n = out.len();
    for (i, (t, orig)) in out.into_iter().enumerate() {
        let orig_last = n_old > 0 && orig == Some(n_old - 1);
        let t = match (orig, i + 1 == n) {
            (Some(_), false) if !orig_last => t,
            (Some(_), true) if orig_last => t,
            (_, false) => format!("{}\n\n", trim_nl(&t)),
            (_, true) => format!("{}\n", trim_nl(&t)),
        };
        text.push_str(&t);
    }
    if !layout.trailer.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&layout.trailer);
    }
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    Ok(Rendered { text, rewritten })
}
