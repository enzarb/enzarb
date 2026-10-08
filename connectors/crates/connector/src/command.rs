//! Command and mention parsing shared by connectors.
//!
//! Text inside fenced code blocks, inline code and block quotes is ignored, so
//! quoting someone else's `/approve` or pasting a log that mentions the bot
//! does not trigger anything.

/// Returns `body` with fenced code blocks, inline code spans and quoted lines
/// (`> ...`) removed. Code spans are replaced by a space.
pub fn strip_code_and_quotes(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    // Open fence: (marker char, length).
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        // Up to 3 spaces of indent make a fence; be lenient and accept any.
        let run = fence_run(trimmed);
        if let Some((ch, len)) = fence {
            if let Some((c, n)) = run
                && c == ch
                && n >= len
                && trimmed[n..].trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some(open) = run {
            fence = Some(open);
            continue;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        out.push_str(&strip_inline_code(line));
        out.push('\n');
    }
    out
}

/// A leading run of 3 or more backticks or tildes: (char, run length).
fn fence_run(s: &str) -> Option<(char, usize)> {
    let ch = s.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = s.chars().take_while(|c| *c == ch).count();
    (n >= 3).then_some((ch, n))
}

fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let ticks = rest[start..].chars().take_while(|c| *c == '`').count();
        let after = &rest[start + ticks..];
        let closer = "`".repeat(ticks);
        // Closing run must be exactly `ticks` long.
        let mut search = 0;
        let mut end = None;
        while let Some(i) = after[search..].find(&closer) {
            let at = search + i;
            let run = after[at..].chars().take_while(|c| *c == '`').count();
            if run == ticks {
                end = Some(at);
                break;
            }
            search = at + run;
        }
        match end {
            Some(at) => {
                out.push(' ');
                rest = &after[at + ticks..];
            }
            None => {
                // Unclosed: treat the backticks literally.
                out.push_str(&rest[start..start + ticks]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// True if a line of `body` is exactly `/approve` (surrounding whitespace and
/// ASCII case ignored), outside code blocks and quotes.
pub fn has_approve_command(body: &str) -> bool {
    strip_code_and_quotes(body)
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("/approve"))
}

fn is_handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// True if `body` mentions `@handle` (case-insensitive) outside code blocks and
/// quotes. `handle` is given without the `@`. `@enzarb-ui` or `a@enzarb.dev`
/// do not count as mentions of `enzarb`; `@enzarb,` and `@enzarb[bot]` do.
pub fn mentions_handle(body: &str, handle: &str) -> bool {
    if handle.is_empty() {
        return false;
    }
    let text = strip_code_and_quotes(body);
    let want: Vec<char> = handle.chars().map(|c| c.to_ascii_lowercase()).collect();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c != '@' {
            continue;
        }
        if i > 0 && (is_handle_char(chars[i - 1]) || chars[i - 1] == '.' || chars[i - 1] == '@') {
            continue;
        }
        let name = &chars[i + 1..];
        if name.len() < want.len() {
            continue;
        }
        let matches = name[..want.len()]
            .iter()
            .zip(&want)
            .all(|(a, b)| a.to_ascii_lowercase() == *b);
        if !matches {
            continue;
        }
        match name.get(want.len()) {
            Some(n) if is_handle_char(*n) => continue,
            _ => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_table() {
        let cases: &[(&str, bool)] = &[
            ("/approve", true),
            ("  /approve  ", true),
            ("/APPROVE", true),
            ("Looks good.\n/approve\nThanks", true),
            ("Looks good\r\n/approve\r\n", true),
            ("/approve please", false),
            ("please /approve", false),
            ("/approved", false),
            ("/approve-all", false),
            ("", false),
            ("approve", false),
            ("> /approve", false),
            ("> quoted\n> /approve\nreply", false),
            (">/approve", false),
            ("```\n/approve\n```", false),
            ("```sh\n/approve\n```\nnothing", false),
            ("~~~\n/approve\n~~~", false),
            ("```\n/approve\n```\n/approve", true),
            ("`/approve`", false),
            ("```\n/approve", false), // unterminated fence swallows the rest
            ("````\n```\n/approve\n```\n````", false),
        ];
        for (body, want) in cases {
            assert_eq!(has_approve_command(body), *want, "body {body:?}");
        }
    }

    #[test]
    fn mention_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("@enzarb", "enzarb", true),
            ("hey @enzarb please look", "enzarb", true),
            ("@Enzarb do it", "enzarb", true),
            ("@enzarb, do it", "enzarb", true),
            ("@enzarb.", "enzarb", true),
            ("(@enzarb)", "enzarb", true),
            ("@enzarb[bot] hi", "enzarb", true),
            ("line one\n@enzarb fix", "enzarb", true),
            ("cc @bob @enzarb", "enzarb", true),
            ("@enzarb-ui please", "enzarb", false),
            ("@enzarbs", "enzarb", false),
            ("@enzarb_x", "enzarb", false),
            ("mail me at bob@enzarb.dev", "enzarb", false),
            ("a@enzarb", "enzarb", false),
            ("@@enzarb", "enzarb", false),
            ("enzarb", "enzarb", false),
            ("@enzar", "enzarb", false),
            ("", "enzarb", false),
            ("@enzarb", "", false),
            ("> @enzarb do it", "enzarb", false),
            ("`@enzarb`", "enzarb", false),
            ("run `x` then @enzarb", "enzarb", true),
            ("``a ` b @enzarb``", "enzarb", false),
            ("```\n@enzarb\n```", "enzarb", false),
            ("```\n@enzarb\n```\n@enzarb", "enzarb", true),
            ("an `unclosed @enzarb", "enzarb", true),
            ("@my-bot go", "my-bot", true),
        ];
        for (body, handle, want) in cases {
            assert_eq!(mentions_handle(body, handle), *want, "body {body:?}");
        }
    }
}
