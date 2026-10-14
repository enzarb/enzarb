//! Text dependencies: `Depends on #12`, `Blocked by owner/repo#12` in an issue, change
//! request or comment body (docs/adr/0017-work-item-dependencies.md).
//!
//! Pure and provider-neutral. Code (fenced blocks and inline spans) and quoted lines are
//! skipped. Several references may follow one keyword (`Depends on #1, #2 and #3`); anything
//! else after it (`depends on the #1 priority`) is prose and yields nothing.

use crate::{RepoKey, WorkItemKey};

/// A reference as written: `#12` has no repo, `owner/repo#12` has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyRef {
    pub repo: Option<(String, String)>,
    pub number: u64,
}

const KEYWORDS: [&str; 2] = ["depends on", "blocked by"];

/// References declared in `text`, in order of appearance, without duplicates.
pub fn parse_dependencies(text: &str) -> Vec<DependencyRef> {
    let mut out: Vec<DependencyRef> = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (Some(open), Some(m)) if open == m => {
                fence = None;
                continue;
            }
            (None, Some(m)) => {
                fence = Some(m);
                continue;
            }
            (Some(_), _) => continue,
            _ => {}
        }
        if trimmed.starts_with('>') {
            continue;
        }
        let line = strip_inline_code(line);
        let lower = line.to_ascii_lowercase();
        for kw in KEYWORDS {
            let mut from = 0;
            while let Some(i) = lower[from..].find(kw) {
                let start = from + i;
                let end = start + kw.len();
                from = end;
                let boundary = lower[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !c.is_alphanumeric());
                if !boundary {
                    continue;
                }
                for r in refs_after(&line[end..]) {
                    if !out.contains(&r) {
                        out.push(r);
                    }
                }
            }
        }
    }
    out
}

/// Resolve parsed references against the repo of the text they came from.
pub fn resolve_dependencies(refs: &[DependencyRef], home: &RepoKey) -> Vec<WorkItemKey> {
    let mut out: Vec<WorkItemKey> = Vec::new();
    for r in refs {
        let repo = match &r.repo {
            Some((owner, repo)) => RepoKey::new(home.connector.clone(), owner, repo),
            None => home.clone(),
        };
        let key = WorkItemKey::new(repo, r.number);
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Parse and resolve in one go.
pub fn text_dependencies(text: &str, home: &RepoKey) -> Vec<WorkItemKey> {
    resolve_dependencies(&parse_dependencies(text), home)
}

fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// The list of references directly after a keyword: `: #1, #2 and org/repo#3`.
fn refs_after(rest: &str) -> Vec<DependencyRef> {
    let mut out = Vec::new();
    let mut rest = rest.trim_start().trim_start_matches(':').trim_start();
    while let Some((r, tail)) = one_ref(rest) {
        out.push(r);
        let t = tail.trim_start();
        let t = if let Some(t) = t.strip_prefix(',').or_else(|| t.strip_prefix('&')) {
            t
        } else if t.len() >= 4 && t.is_char_boundary(3) && t[..3].eq_ignore_ascii_case("and") {
            match t[3..].chars().next() {
                Some(c) if c.is_whitespace() => &t[3..],
                _ => break,
            }
        } else {
            break;
        };
        rest = t.trim_start();
    }
    out
}

fn one_ref(s: &str) -> Option<(DependencyRef, &str)> {
    let hash = s.find('#')?;
    let head = &s[..hash];
    let repo = if head.is_empty() {
        None
    } else {
        let (owner, repo) = head.split_once('/')?;
        let ok = |p: &str| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !ok(owner) || !ok(repo) {
            return None;
        }
        Some((owner.to_string(), repo.to_string()))
    };
    let digits: String = s[hash + 1..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let number: u64 = digits.parse().ok().filter(|n| *n > 0)?;
    let tail = &s[hash + 1 + digits.len()..];
    // `#12abc` is not a reference.
    if tail.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }
    Some((DependencyRef { repo, number }, tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(text: &str) -> Vec<u64> {
        parse_dependencies(text).iter().map(|r| r.number).collect()
    }

    #[test]
    fn simple_forms() {
        assert_eq!(nums("Depends on #12"), [12]);
        assert_eq!(nums("blocked by #7."), [7]);
        assert_eq!(nums("DEPENDS ON: #3"), [3]);
        assert_eq!(nums("- Blocked by #4\n- depends on #5"), [4, 5]);
    }

    #[test]
    fn lists() {
        assert_eq!(nums("Depends on #1, #2 and #3"), [1, 2, 3]);
        assert_eq!(nums("Blocked by #1 & #2"), [1, 2]);
        assert_eq!(nums("Depends on #1 AND #2, #3"), [1, 2, 3]);
    }

    #[test]
    fn cross_repo() {
        let r = parse_dependencies("Depends on enzarb/enzarb#12 and #3");
        assert_eq!(
            r,
            [
                DependencyRef {
                    repo: Some(("enzarb".into(), "enzarb".into())),
                    number: 12
                },
                DependencyRef {
                    repo: None,
                    number: 3
                },
            ]
        );
    }

    #[test]
    fn duplicates_collapse() {
        assert_eq!(nums("Depends on #1, #1\nBlocked by #1"), [1]);
    }

    #[test]
    fn code_and_quotes_are_skipped() {
        assert!(nums("```\nDepends on #1\n```").is_empty());
        assert!(nums("~~~\nBlocked by #1\n~~~").is_empty());
        assert!(nums("`Depends on #1`").is_empty());
        assert!(nums("> Depends on #1").is_empty());
        assert_eq!(nums("```\nx\n```\nDepends on #2"), [2]);
    }

    #[test]
    fn prose_is_not_a_dependency() {
        assert!(nums("This depends on the #1 priority").is_empty());
        assert!(nums("Depends on nothing").is_empty());
        assert!(nums("Depends on #abc").is_empty());
        assert!(nums("Depends on #12abc").is_empty());
        assert!(nums("Depends on #0").is_empty());
        assert!(nums("undepends on #1").is_empty());
        assert!(nums("Depends on a/b/c#1").is_empty());
    }

    #[test]
    fn resolve_uses_home_repo() {
        let home = RepoKey::new("github", "o", "r");
        let keys = text_dependencies("Depends on #1, other/repo#2, o/r#1", &home);
        let want = [
            WorkItemKey::new(home.clone(), 1),
            WorkItemKey::new(RepoKey::new("github", "other", "repo"), 2),
        ];
        assert_eq!(keys, want);
    }
}
