//! Finds the Project that owns a repository (Project CRs, read-only).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use enzarb_crds::Project;
use kube::{Api, Client, ResourceExt, api::ListParams};

/// The Project that owns a repository, and the repository's default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub name: String,
    pub default_branch: String,
}

#[async_trait]
pub trait ProjectLookup: Send + Sync {
    /// The Project owning lowercase `owner/repo`; `Err` is transient (the API server).
    async fn owner(&self, repo: &str) -> Result<Option<ProjectRef>, String>;
}

/// `https://github.com/Owner/Repo(.git)` to the lowercase `owner/repo` key the receiver stores.
pub fn repo_key(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let path = rest.split_once('/')?.1;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut it = path.split('/');
    let (o, r) = (it.next()?, it.next()?);
    if o.is_empty() || r.is_empty() || it.next().is_some() {
        return None;
    }
    Some(format!("{o}/{r}").to_ascii_lowercase())
}

/// The owner among `(project, repositories)` pairs; the alphabetically first Project wins when
/// several list the repository (the second return value is how many did).
pub fn find_owner<'a>(
    projects: impl IntoIterator<Item = (&'a str, Vec<(&'a str, &'a str)>)>,
    repo: &str,
) -> (Option<ProjectRef>, usize) {
    let mut hits: Vec<ProjectRef> = Vec::new();
    for (name, repos) in projects {
        for (url, branch) in repos {
            if repo_key(url).as_deref() == Some(repo) {
                hits.push(ProjectRef {
                    name: name.to_string(),
                    default_branch: if branch.is_empty() {
                        "main".into()
                    } else {
                        branch.into()
                    },
                });
                break;
            }
        }
    }
    hits.sort_by(|a, b| a.name.cmp(&b.name));
    let n = hits.len();
    (hits.into_iter().next(), n)
}

/// Lists Projects through the API server, with a short cache (events arrive in bursts).
pub struct KubeProjects {
    api: Api<Project>,
    ttl: Duration,
    cache: Mutex<Option<(Instant, Vec<Project>)>>,
}

impl KubeProjects {
    pub async fn new() -> anyhow::Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            api: Api::all(client),
            ttl: Duration::from_secs(10),
            cache: Mutex::new(None),
        })
    }

    async fn projects(&self) -> Result<Vec<Project>, String> {
        if let Some((at, p)) = &*self.cache.lock().unwrap()
            && at.elapsed() < self.ttl
        {
            return Ok(p.clone());
        }
        let list = self
            .api
            .list(&ListParams::default())
            .await
            .map_err(|e| format!("listing Projects failed: {e}"))?
            .items;
        *self.cache.lock().unwrap() = Some((Instant::now(), list.clone()));
        Ok(list)
    }
}

#[async_trait]
impl ProjectLookup for KubeProjects {
    async fn owner(&self, repo: &str) -> Result<Option<ProjectRef>, String> {
        let list = self.projects().await?;
        let names: Vec<String> = list.iter().map(|p| p.name_any()).collect();
        let input = list.iter().zip(&names).map(|(p, n)| {
            (
                n.as_str(),
                p.spec
                    .repositories
                    .iter()
                    .map(|r| (r.url.as_str(), r.default_branch.as_str()))
                    .collect(),
            )
        });
        let (owner, n) = find_owner(input, repo);
        if n > 1 {
            tracing::warn!(
                repo,
                projects = n,
                "several Projects list this repository; using the first by name"
            );
        }
        Ok(owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_keys() {
        assert_eq!(
            repo_key("https://github.com/Enzarb/App.git").as_deref(),
            Some("enzarb/app")
        );
        assert_eq!(repo_key("https://github.com/o/r/").as_deref(), Some("o/r"));
        assert_eq!(repo_key("https://github.com/o"), None);
        assert_eq!(repo_key("nonsense"), None);
    }

    #[test]
    fn owner_is_first_by_name_with_default_branch() {
        let input = vec![
            ("zeta", vec![("https://github.com/o/r", "main")]),
            (
                "alpha",
                vec![
                    ("https://github.com/x/y", "main"),
                    ("https://github.com/O/R", "trunk"),
                ],
            ),
        ];
        let (o, n) = find_owner(input.clone(), "o/r");
        assert_eq!(n, 2);
        assert_eq!(
            o,
            Some(ProjectRef {
                name: "alpha".into(),
                default_branch: "trunk".into()
            })
        );
        assert_eq!(find_owner(input, "none/x"), (None, 0));
    }
}
