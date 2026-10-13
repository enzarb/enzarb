use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid key {input:?}: {reason}")]
pub struct KeyError {
    pub input: String,
    pub reason: &'static str,
}

fn err(input: &str, reason: &'static str) -> KeyError {
    KeyError {
        input: input.to_string(),
        reason,
    }
}

/// `<connector>:<owner>/<repo>`, e.g. `github:enzarb/enzarb-ui`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoKey {
    pub connector: String,
    pub owner: String,
    pub repo: String,
}

impl RepoKey {
    pub fn new(
        connector: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            connector: connector.into(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    /// `owner/repo`, without the connector prefix.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

fn valid_part(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '#', ':', ' ', '\t', '\n'])
}

impl FromStr for RepoKey {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, KeyError> {
        let (connector, rest) = s
            .split_once(':')
            .ok_or_else(|| err(s, "missing ':' after connector"))?;
        let (owner, repo) = rest
            .split_once('/')
            .ok_or_else(|| err(s, "missing '/' between owner and repo"))?;
        if !valid_part(connector) {
            return Err(err(s, "bad connector"));
        }
        if !valid_part(owner) {
            return Err(err(s, "bad owner"));
        }
        if !valid_part(repo) {
            return Err(err(s, "bad repo"));
        }
        Ok(Self::new(connector, owner, repo))
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}/{}", self.connector, self.owner, self.repo)
    }
}

/// `<repo key>#<number>`, e.g. `github:enzarb/enzarb-ui#42`. Issues and
/// change requests share one number space.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkItemKey {
    pub repo: RepoKey,
    pub number: u64,
}

impl WorkItemKey {
    pub fn new(repo: RepoKey, number: u64) -> Self {
        Self { repo, number }
    }
}

impl FromStr for WorkItemKey {
    type Err = KeyError;

    fn from_str(s: &str) -> Result<Self, KeyError> {
        let (repo, number) = s
            .rsplit_once('#')
            .ok_or_else(|| err(s, "missing '#number'"))?;
        let repo: RepoKey = repo.parse().map_err(|_| err(s, "bad repo key"))?;
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err(s, "bad number"));
        }
        let number = number.parse().map_err(|_| err(s, "number out of range"))?;
        Ok(Self { repo, number })
    }
}

impl fmt::Display for WorkItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

macro_rules! string_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
string_serde!(RepoKey);
string_serde!(WorkItemKey);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_key_round_trip() {
        for s in ["github:enzarb/enzarb-ui", "gitlab:grp/proj.name", "x:a/b"] {
            let k: RepoKey = s.parse().unwrap();
            assert_eq!(k.to_string(), s);
        }
        let k: RepoKey = "github:enzarb/enzarb-ui".parse().unwrap();
        assert_eq!(k, RepoKey::new("github", "enzarb", "enzarb-ui"));
        assert_eq!(k.full_name(), "enzarb/enzarb-ui");
    }

    #[test]
    fn work_item_key_round_trip() {
        for s in [
            "github:enzarb/enzarb-ui#42",
            "github:o/r#0",
            "forgejo:a/b#18446744073709551615",
        ] {
            let k: WorkItemKey = s.parse().unwrap();
            assert_eq!(k.to_string(), s);
        }
        let k: WorkItemKey = "github:org/repo#42".parse().unwrap();
        assert_eq!(k.number, 42);
        assert_eq!(k.repo.to_string(), "github:org/repo");
    }

    #[test]
    fn repo_key_rejects() {
        for s in [
            "",
            "github",
            "github:",
            "github:owner",
            "github:owner/",
            "github:/repo",
            ":o/r",
            "github:o/r/extra",
            "github:o/r#1",
            "github:o /r",
        ] {
            assert!(s.parse::<RepoKey>().is_err(), "{s:?} should fail");
        }
    }

    #[test]
    fn work_item_key_rejects() {
        for s in [
            "",
            "github:o/r",
            "github:o/r#",
            "github:o/r#x",
            "github:o/r#-1",
            "github:o/r#1#2",
            "#1",
            "github:o/r#99999999999999999999",
        ] {
            assert!(s.parse::<WorkItemKey>().is_err(), "{s:?} should fail");
        }
    }

    #[test]
    fn serde_as_strings() {
        let k: WorkItemKey = "github:a/b#7".parse().unwrap();
        let j = serde_json::to_string(&k).unwrap();
        assert_eq!(j, "\"github:a/b#7\"");
        assert_eq!(serde_json::from_str::<WorkItemKey>(&j).unwrap(), k);
        assert!(serde_json::from_str::<RepoKey>("\"nope\"").is_err());
    }
}
