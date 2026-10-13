//! Pure decisions: which webhook deliveries matter and whether a version is an upgrade.

use semver::Version;
use serde::Deserialize;

pub const REPO: &str = "enzarb/enzarb";

/// Chart releases in the monorepo are tagged `helm/vX.Y.Z`; other components' tags are ignored.
pub const TAG_PREFIX: &str = "helm/";

#[derive(Deserialize)]
pub struct ReleaseEvent {
    pub action: Option<String>,
    pub repository: Option<Repository>,
    pub release: Option<Release>,
}

#[derive(Deserialize)]
pub struct Repository {
    pub full_name: String,
}

#[derive(Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, PartialEq)]
pub enum Filtered {
    Act(Version),
    Ignore(&'static str),
}

/// Parse a strict `helm/vX.Y.Z` release tag (no prerelease or build metadata).
pub fn parse_release_tag(tag: &str) -> Option<Version> {
    parse_tag(tag.strip_prefix(TAG_PREFIX)?)
}

/// Parse a strict `vX.Y.Z` tag (no prerelease or build metadata).
pub fn parse_tag(tag: &str) -> Option<Version> {
    let v = Version::parse(tag.strip_prefix('v')?).ok()?;
    (v.pre.is_empty() && v.build.is_empty()).then_some(v)
}

pub fn filter(ev: &ReleaseEvent) -> Filtered {
    if ev.action.as_deref() != Some("published") {
        return Filtered::Ignore("action is not published");
    }
    if ev.repository.as_ref().map(|r| r.full_name.as_str()) != Some(REPO) {
        return Filtered::Ignore("repository is not enzarb/enzarb");
    }
    let Some(rel) = &ev.release else {
        return Filtered::Ignore("no release in payload");
    };
    if rel.prerelease || rel.draft {
        return Filtered::Ignore("prerelease or draft");
    }
    match parse_release_tag(&rel.tag_name) {
        Some(v) => Filtered::Act(v),
        None => Filtered::Ignore("tag is not helm/vX.Y.Z"),
    }
}

#[derive(Debug, PartialEq)]
pub enum Upgrade {
    Patch,
    Equal,
    Older,
}

/// Compare a release against the current `spec.version` (an optional leading `v` is accepted).
/// A missing current version is patched. Unparsable current versions are an error.
pub fn compare(new: &Version, current: Option<&str>) -> Result<Upgrade, String> {
    let Some(cur) = current.filter(|c| !c.is_empty()) else {
        return Ok(Upgrade::Patch);
    };
    let cur = Version::parse(cur.strip_prefix('v').unwrap_or(cur))
        .map_err(|e| format!("current spec.version {cur:?} is not semver: {e}"))?;
    // Full semver ordering: a prerelease current (1.2.3-rc.1) upgrades to 1.2.3, nothing moves backwards.
    Ok(match new.cmp(&cur) {
        std::cmp::Ordering::Greater => Upgrade::Patch,
        std::cmp::Ordering::Equal => Upgrade::Equal,
        std::cmp::Ordering::Less => Upgrade::Older,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(action: &str, repo: &str, tag: &str, pre: bool) -> ReleaseEvent {
        ReleaseEvent {
            action: Some(action.into()),
            repository: Some(Repository {
                full_name: repo.into(),
            }),
            release: Some(Release {
                tag_name: tag.into(),
                prerelease: pre,
                draft: false,
            }),
        }
    }

    #[test]
    fn accepts_published_release() {
        let f = filter(&ev("published", REPO, "helm/v1.2.3", false));
        assert_eq!(f, Filtered::Act(Version::new(1, 2, 3)));
    }

    #[test]
    fn rejects_other_actions_repos_and_tags() {
        for e in [
            ev("created", REPO, "helm/v1.2.3", false),
            ev("published", "enzarb/other", "helm/v1.2.3", false),
            ev("published", "enzarb/enzarb-helm", "helm/v1.2.3", false),
            ev("published", REPO, "v1.2.3", false),
            ev("published", REPO, "ui/v1.2.3", false),
            ev("published", REPO, "helm/1.2.3", false),
            ev("published", REPO, "helm/v1.2", false),
            ev("published", REPO, "helm/v1.2.3-rc.1", false),
            ev("published", REPO, "helm/v1.2.3+b", false),
            ev("published", REPO, "helm/v1.2.3", true),
        ] {
            assert!(matches!(filter(&e), Filtered::Ignore(_)));
        }
    }

    #[test]
    fn compare_never_downgrades() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert_eq!(compare(&v("1.2.4"), Some("1.2.3")), Ok(Upgrade::Patch));
        assert_eq!(compare(&v("1.10.0"), Some("1.9.9")), Ok(Upgrade::Patch));
        assert_eq!(compare(&v("1.2.3"), Some("1.2.3")), Ok(Upgrade::Equal));
        assert_eq!(compare(&v("1.2.2"), Some("1.2.3")), Ok(Upgrade::Older));
        assert_eq!(compare(&v("1.2.3"), Some("v1.2.3")), Ok(Upgrade::Equal));
        assert_eq!(compare(&v("1.2.3"), Some("1.2.3-rc.1")), Ok(Upgrade::Patch));
        assert_eq!(compare(&v("1.0.0"), None), Ok(Upgrade::Patch));
        assert!(compare(&v("1.0.0"), Some("latest")).is_err());
    }
}
