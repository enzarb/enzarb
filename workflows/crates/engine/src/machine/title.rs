//! Conventional Commit titles for the pull requests enzarb opens.
//!
//! `release.yml` derives versions from squash-merge titles, so a PR title that is not a
//! Conventional Commit with a scope releases nothing. The implement agent proposes a
//! title; [`pr_title`] validates it and falls back to a derived one.

/// Allowed commit types. Keep in sync with the types `release.yml` understands.
const TYPES: &[&str] = &[
    "feat", "fix", "perf", "revert", "docs", "chore", "refactor", "test", "build", "ci", "style",
];

/// Maximum title length in characters.
const MAX_LEN: usize = 100;

fn valid_scope(scope: &str) -> bool {
    let mut chars = scope.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | '-')
        })
}

/// Whether `title` is `type(scope)!?: description` on a single line of at most 100 chars.
pub fn is_conventional_title(title: &str) -> bool {
    if title.contains(['\n', '\r']) || title.chars().count() > MAX_LEN {
        return false;
    }
    let Some((ty, rest)) = title.split_once('(') else {
        return false;
    };
    if !TYPES.contains(&ty) {
        return false;
    }
    let Some((scope, rest)) = rest.split_once(')') else {
        return false;
    };
    if !valid_scope(scope) {
        return false;
    }
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    match rest.strip_prefix(": ") {
        Some(desc) => !desc.trim().is_empty(),
        None => false,
    }
}

/// The scope for a repo: the name without owner or platform, minus any `enzarb-` prefix.
fn scope_of(repo: &str) -> String {
    let name = repo.rsplit(['/', ':']).next().unwrap_or("");
    let name = name.strip_prefix("enzarb-").unwrap_or(name);
    let scope: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let scope = scope.trim_matches(['-', '.', '_']).to_string();
    if valid_scope(&scope) {
        scope
    } else {
        "repo".into()
    }
}

/// Strip a leading `type(scope)!:` / `type:` / `bug:` prefix so it is not doubled.
fn strip_prefix_word(title: &str) -> &str {
    let Some((head, rest)) = title.split_once(':') else {
        return title;
    };
    let word = head.split('(').next().unwrap_or("").to_lowercase();
    let word = word.trim_end_matches('!');
    let looks_like_prefix = head.chars().count() <= 40
        && !head.contains(char::is_whitespace)
        && (TYPES.contains(&word) || word == "bug" || word == "bugfix");
    if looks_like_prefix { rest } else { title }
}

/// Derive a valid Conventional Commit title from the issue title and repo name.
pub fn derive_title(issue_title: &str, repo_name: &str) -> String {
    let trimmed = issue_title.trim();
    let lower = trimmed.to_lowercase();
    let ty = if ["fix", "bug"].iter().any(|p| lower.starts_with(p)) {
        "fix"
    } else {
        "feat"
    };
    let scope = scope_of(repo_name);
    let prefix = format!("{ty}({scope}): ");

    let desc = strip_prefix_word(trimmed);
    let desc = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = desc.chars();
    let desc = match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect::<String>(),
        None => String::new(),
    };
    let room = MAX_LEN.saturating_sub(prefix.chars().count());
    let desc: String = desc.chars().take(room).collect();
    let desc = desc.trim_end();
    let desc = if desc.is_empty() { "update" } else { desc };
    format!("{prefix}{desc}")
}

/// The agent's title if it is a valid Conventional Commit, otherwise a derived one.
pub fn pr_title(agent: Option<&str>, issue_title: &str, repo_name: &str) -> String {
    match agent.map(str::trim) {
        Some(t) if is_conventional_title(t) => t.to_string(),
        _ => derive_title(issue_title, repo_name),
    }
}

/// Types accepted as-is for a squash title (`^(feat|fix|docs|chore|refactor|test|ci|build|perf|style)`).
const SQUASH_TYPES: &[&str] = &[
    "feat", "fix", "docs", "chore", "refactor", "test", "ci", "build", "perf", "style",
];

/// `^(feat|fix|...)(\(.+\))?!?: .+`
fn is_squash_title(title: &str) -> bool {
    if title.contains(['\n', '\r']) {
        return false;
    }
    let ty_end = title
        .find(|c: char| !c.is_ascii_lowercase())
        .unwrap_or(title.len());
    if !SQUASH_TYPES.contains(&&title[..ty_end]) {
        return false;
    }
    let mut rest = &title[ty_end..];
    if let Some(r) = rest.strip_prefix('(') {
        let Some(close) = r.find(')') else {
            return false;
        };
        if close == 0 {
            return false;
        }
        rest = &r[close + 1..];
    }
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    matches!(rest.strip_prefix(": "), Some(d) if !d.is_empty())
}

/// The commit title for squash-merging a PR: its own title if that is already a
/// Conventional Commit, otherwise `type: <title>` with the type taken from the head branch
/// prefix (`feat/...`, `fix/...`) and `chore` when the branch says nothing.
pub fn squash_title(pr_title: &str, head_ref: &str) -> String {
    let trimmed = pr_title.trim();
    if is_squash_title(trimmed) {
        return trimmed.to_string();
    }
    let prefix = head_ref.split('/').next().unwrap_or("");
    let ty = match prefix {
        p if head_ref.contains('/') && SQUASH_TYPES.contains(&p) => p,
        "bug" | "bugfix" | "hotfix" if head_ref.contains('/') => "fix",
        _ => "chore",
    };
    let desc = strip_prefix_word(trimmed).trim();
    let desc = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    let desc = if desc.is_empty() {
        "update".to_string()
    } else {
        desc
    };
    format!("{ty}: {desc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_conventional_titles() {
        assert!(is_conventional_title("fix(operator): x"));
        assert!(is_conventional_title("feat(ui)!: x"));
        assert!(is_conventional_title("perf(a/b): x"));
    }

    #[test]
    fn rejects_bad_titles() {
        for t in [
            "fix: x",
            "fix(): x",
            "wip(ui): x",
            "fix(ui): ",
            "fix(ui):   ",
            "fix(ui) x",
            "fix(ui):x",
            "fix(Ui): x",
            "fix(a b): x",
            "fix(ui): x\ny",
            "Use Conventional Commit titles for PRs enzarb opens",
        ] {
            assert!(!is_conventional_title(t), "{t:?}");
        }
        let long = format!("fix(ui): {}", "a".repeat(100));
        assert!(!is_conventional_title(&long));
        let max = format!("fix(ui): {}", "a".repeat(91));
        assert_eq!(max.chars().count(), 100);
        assert!(is_conventional_title(&max));
    }

    #[test]
    fn derives_type_and_scope() {
        assert_eq!(
            derive_title("Add dark mode", "enzarb/enzarb-workflows"),
            "feat(workflows): add dark mode"
        );
        assert_eq!(
            derive_title("Bug: crash on start", "github:enzarb/enzarb-ui"),
            "fix(ui): crash on start"
        );
        assert_eq!(
            derive_title("Fixes the thing", "o/x"),
            "fix(x): fixes the thing"
        );
        assert_eq!(derive_title("Add x", "enzarb"), "feat(enzarb): add x");
        assert_eq!(derive_title("Add x", "enzarb-"), "feat(repo): add x");
        assert_eq!(derive_title("Add x", ""), "feat(repo): add x");
    }

    #[test]
    fn does_not_double_prefix() {
        assert_eq!(derive_title("feat(ui): Add x", "o/r"), "feat(r): add x");
        assert_eq!(derive_title("fix: Handle y", "o/r"), "fix(r): handle y");
        assert_eq!(derive_title("fix(a)!: Handle y", "o/r"), "fix(r): handle y");
        // Not a prefix: contains a colon later in a normal sentence.
        assert_eq!(
            derive_title("Support the foo: bar", "o/r"),
            "feat(r): support the foo: bar"
        );
    }

    #[test]
    fn truncates_and_handles_odd_input() {
        let long = "é".repeat(500);
        let t = derive_title(&long, "o/enzarb-workflows");
        assert!(t.chars().count() <= MAX_LEN);
        assert!(is_conventional_title(&t));
        assert_eq!(derive_title("", "o/r"), "feat(r): update");
        assert_eq!(derive_title("  \n ", "o/r"), "feat(r): update");
        assert_eq!(derive_title("a\nb", "o/r"), "feat(r): a b");
    }

    #[test]
    fn derived_titles_always_validate() {
        for issue in [
            "Add dark mode",
            "",
            "   ",
            "fix:",
            "feat(x):",
            "Ünïcode ✨ title",
            "line one\nline two",
            "Bug",
            &"x".repeat(300),
        ] {
            for repo in ["enzarb/enzarb-workflows", "", "a/B C", "github:o/ÜÜ", "/"] {
                let t = derive_title(issue, repo);
                assert!(is_conventional_title(&t), "{issue:?} {repo:?} -> {t:?}");
            }
        }
    }

    #[test]
    fn pr_title_prefers_valid_agent_title() {
        assert_eq!(
            pr_title(Some("  fix(op): a  "), "Issue", "o/enzarb-x"),
            "fix(op): a"
        );
        assert_eq!(
            pr_title(Some("bad"), "Issue", "o/enzarb-x"),
            "feat(x): issue"
        );
        assert_eq!(pr_title(None, "Issue", "o/enzarb-x"), "feat(x): issue");
    }

    #[test]
    fn squash_title_table() {
        for (title, head, want) in [
            // already conventional: kept, with or without a scope or `!`
            ("feat(ui): dark mode", "x/y", "feat(ui): dark mode"),
            ("fix: crash", "feat/x", "fix: crash"),
            ("refactor(a)!: drop it", "", "refactor(a)!: drop it"),
            ("  docs: readme  ", "", "docs: readme"),
            // derived from the branch prefix
            ("Dark mode", "feat/dark", "feat: Dark mode"),
            ("Crash on start", "fix/crash", "fix: Crash on start"),
            ("Crash on start", "bugfix/crash", "fix: Crash on start"),
            // chore fallback
            ("Dark mode", "enzarb/issue-30", "chore: Dark mode"),
            ("Dark mode", "feat", "chore: Dark mode"),
            ("Dark mode", "", "chore: Dark mode"),
            // invalid prefixes do not count
            ("feat(): x", "", "chore: x"),
            ("feature: x", "", "chore: feature: x"),
            ("feat:x", "", "chore: x"),
            ("feat: ", "", "chore: update"),
            ("revert: x", "", "chore: x"),
            ("", "", "chore: update"),
        ] {
            assert_eq!(squash_title(title, head), want, "{title:?} {head:?}");
        }
    }
}
