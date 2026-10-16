//! Pure helpers of the `prepare_request` handler (`agent.rs`): parsing git
//! output and capping the reply. The git calls themselves live next to the
//! run's other git code.
//!
//! The reply is best-effort: a step that fails leaves its field empty.

/// Most changed files reported; `changed_files_total` carries the true count.
pub const MAX_FILES: usize = 500;
/// Most bytes of file names reported.
pub const MAX_BYTES: usize = 64 * 1024;
/// Most affected components reported (the repo has a dozen).
pub const MAX_AFFECTED: usize = 64;
/// The repo script that maps changed files to components.
pub const AFFECTED_SCRIPT: &str = "scripts/monorepo/affected.sh";

/// File names from `git diff --name-only -z` output (NUL separated, so odd
/// names survive). Lossy for non-UTF-8 names; empty entries are dropped.
pub fn parse_names(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|n| !n.is_empty())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect()
}

/// The first [`MAX_FILES`] names that fit in [`MAX_BYTES`], and the true total.
pub fn cap_files(mut files: Vec<String>) -> (Vec<String>, usize) {
    let total = files.len();
    let mut bytes = 0;
    let keep = files
        .iter()
        .take(MAX_FILES)
        .take_while(|f| {
            bytes += f.len() + 1;
            bytes <= MAX_BYTES
        })
        .count();
    files.truncate(keep);
    (files, total)
}

/// Component names printed by `affected.sh`, one per line.
pub fn parse_affected(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && l.len() <= 64
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .take(MAX_AFFECTED)
        .map(String::from)
        .collect()
}

/// A full object name from `git merge-base` output.
pub fn parse_sha(stdout: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(stdout);
    let s = s.trim();
    (s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_split_on_nul() {
        assert_eq!(
            parse_names(b"a/b.rs\0with space.md\0\0new\nline\0"),
            vec!["a/b.rs", "with space.md", "new\nline"]
        );
        assert!(parse_names(b"").is_empty());
    }

    #[test]
    fn the_list_is_capped_by_count_with_the_true_total() {
        let files: Vec<String> = (0..MAX_FILES + 10).map(|i| format!("f{i}")).collect();
        let (kept, total) = cap_files(files);
        assert_eq!(kept.len(), MAX_FILES);
        assert_eq!(total, MAX_FILES + 10);
        assert_eq!(kept[0], "f0");
    }

    #[test]
    fn the_list_is_capped_by_bytes() {
        let long = "x".repeat(10 * 1024);
        let files = vec![long.clone(); 10];
        let (kept, total) = cap_files(files);
        assert_eq!(total, 10);
        assert_eq!(kept.len(), 6);
        assert!(kept.iter().map(|f| f.len() + 1).sum::<usize>() <= MAX_BYTES);
    }

    #[test]
    fn a_short_list_is_kept_whole() {
        let (kept, total) = cap_files(vec!["a".into(), "b".into()]);
        assert_eq!((kept, total), (vec!["a".to_string(), "b".to_string()], 2));
    }

    #[test]
    fn affected_lines_are_component_names_only() {
        assert_eq!(
            parse_affected(b"proto\n sandbox-container \n\nbad name\n$(x)\nui\n"),
            vec!["proto", "sandbox-container", "ui"]
        );
    }

    #[test]
    fn only_a_full_hex_sha_is_a_merge_base() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(parse_sha(format!("{sha}\n").as_bytes()), Some(sha.into()));
        assert_eq!(parse_sha(b""), None);
        assert_eq!(parse_sha(b"fatal: no merge base\n"), None);
    }
}
