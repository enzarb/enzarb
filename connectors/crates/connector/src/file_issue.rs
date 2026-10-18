//! Naming of the issues the `file_issue` action files: the dedupe key the engine hands to
//! [`Connector::upsert_issue`](crate::Connector::upsert_issue), and its hidden body marker.

/// Start of the hidden idempotency marker a provider embeds in an issue body.
pub const IDEM_MARKER_PREFIX: &str = "<!-- enzarb:idem:";

/// Prefix of every dedupe key made by [`file_issue_key`].
pub const FILE_ISSUE_KEY_PREFIX: &str = "file_issue:";

/// The connector dedupe key for `key` in `repo`: stable across runs and jobs, and distinct
/// from the per-job keys `create_issues` uses.
pub fn file_issue_key(repo: &str, key: &str) -> String {
    format!("{FILE_ISSUE_KEY_PREFIX}{repo}:{key}")
}

/// Whether `body` carries the marker of an issue filed by `file_issue`.
pub fn has_file_issue_marker(body: &str) -> bool {
    body.contains(&format!("{IDEM_MARKER_PREFIX}{FILE_ISSUE_KEY_PREFIX}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_detection() {
        let k = file_issue_key("o/r", "k1");
        assert_eq!(k, "file_issue:o/r:k1");
        assert!(has_file_issue_marker(&format!(
            "x\n\n<!-- enzarb:idem:{k} -->"
        )));
        assert!(!has_file_issue_marker("<!-- enzarb:idem:epic-1-a -->"));
    }
}
