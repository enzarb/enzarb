//! Hidden markers on sub-issues that enzarb creates for an epic.
//!
//! * `<!-- enzarb:epic=<parent work item key>:<key> -->` names the parent epic and the
//!   sub-issue's key in the approved plan.
//! * `<!-- enzarb:epic-depth=<n> -->` is the nesting depth (the parent's depth plus one). It is
//!   informational: the engine takes the depth from the parent's stored state, never from the
//!   body, because anyone can edit a body.

use crate::WorkItemKey;

const EPIC_PREFIX: &str = "<!-- enzarb:epic=";
const DEPTH_PREFIX: &str = "<!-- enzarb:epic-depth=";
const SUFFIX: &str = " -->";

/// The marker tying a sub-issue to `parent` and its plan key. `key` must not contain `-->` or
/// a newline (the plan validator guarantees this).
pub fn epic_marker(parent: &WorkItemKey, key: &str) -> String {
    format!("{EPIC_PREFIX}{parent}:{key}{SUFFIX}")
}

pub fn depth_marker(depth: u32) -> String {
    format!("{DEPTH_PREFIX}{depth}{SUFFIX}")
}

/// `(parent key, sub-issue key)` of the first epic marker in `body`. Keys are split at the last
/// `:`, so the parent key may itself contain colons (`github:owner/repo#1`).
pub fn parse_epic_marker(body: &str) -> Option<(WorkItemKey, String)> {
    let start = body.find(EPIC_PREFIX)? + EPIC_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(SUFFIX)?;
    let inner = &rest[..end];
    let (parent, key) = inner.rsplit_once(':')?;
    if key.is_empty() || key.contains('\n') {
        return None;
    }
    Some((parent.parse().ok()?, key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RepoKey;

    #[test]
    fn marker_round_trips() {
        let parent = WorkItemKey::new(RepoKey::new("github", "enzarb", "enzarb"), 134);
        let body = format!(
            "text\n\n{}\n{}",
            epic_marker(&parent, "api"),
            depth_marker(1)
        );
        assert_eq!(parse_epic_marker(&body), Some((parent, "api".to_string())));
        assert!(body.contains("<!-- enzarb:epic-depth=1 -->"));
    }

    #[test]
    fn malformed_markers_are_ignored() {
        assert_eq!(parse_epic_marker("no marker"), None);
        assert_eq!(parse_epic_marker("<!-- enzarb:epic=nonsense -->"), None);
        assert_eq!(
            parse_epic_marker("<!-- enzarb:epic=github:a/b#1: -->"),
            None
        );
        assert_eq!(parse_epic_marker("<!-- enzarb:epic=github:a/b#1:k"), None);
    }
}
