//! Connector lookup by name (`RepoKey::connector`, e.g. `github`).

use std::collections::HashMap;
use std::sync::Arc;

use enzarb_connector::Connector;

#[derive(Clone, Default)]
pub struct ConnectorRegistry {
    map: HashMap<String, Arc<dyn Connector>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, c: Arc<dyn Connector>) -> Self {
        self.map.insert(name.to_ascii_lowercase(), c);
        self
    }

    /// Registry with the GitHub connector built from `GITHUB_APP_ID`,
    /// `GITHUB_APP_PRIVATE_KEY`, `ENZARB_BOT_HANDLE` and `ENZARB_BOT_LOGIN`.
    pub fn github_from_env() -> Result<Self, enzarb_connector_github::ConfigError> {
        let gh = enzarb_connector_github::GitHubConnector::from_env()?;
        Ok(Self::new().with("github", Arc::new(gh)))
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Connector>> {
        self.map.get(&name.to_ascii_lowercase())
    }
}
