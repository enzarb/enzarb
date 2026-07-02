//! Lightweight session index: `project-agent` persists only session
//! identity/metadata here. Message history is never stored — it's always
//! re-derived from Claude's own on-disk JSONL transcript via ACP `session/load`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::init::home_dir;

const STATE_FILE: &str = ".enzarb/agent_sessions.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Live,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModeInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: SessionStatus,
    #[serde(default)]
    pub mode_id: Option<String>,
    #[serde(default)]
    pub available_modes: Vec<SessionModeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct State {
    sessions: Vec<SessionMeta>,
}

#[derive(Clone)]
pub struct SessionIndex {
    records: Arc<RwLock<HashMap<String, SessionMeta>>>,
    state_path: PathBuf,
}

impl SessionIndex {
    pub async fn load_or_create() -> Result<Self> {
        let state_path = home_dir().join(STATE_FILE);
        let mut map = HashMap::new();

        if state_path.exists() {
            let data = tokio::fs::read(&state_path).await?;
            if let Ok(state) = serde_json::from_slice::<State>(&data) {
                for s in state.sessions {
                    map.insert(s.id.clone(), s);
                }
            }
        }

        Ok(Self {
            records: Arc::new(RwLock::new(map)),
            state_path,
        })
    }

    pub async fn insert(
        &self,
        id: String,
        label: String,
        mode_id: Option<String>,
        available_modes: Vec<SessionModeInfo>,
    ) -> Result<SessionMeta> {
        let now = Utc::now();
        let meta = SessionMeta {
            id: id.clone(),
            label,
            created_at: now,
            updated_at: now,
            status: SessionStatus::Live,
            mode_id,
            available_modes,
        };
        self.records.write().await.insert(id, meta.clone());
        self.persist().await?;
        Ok(meta)
    }

    pub async fn touch(&self, id: &str, status: SessionStatus) -> Result<()> {
        {
            let mut records = self.records.write().await;
            if let Some(m) = records.get_mut(id) {
                m.status = status;
                m.updated_at = Utc::now();
            }
        }
        self.persist().await
    }

    pub async fn set_modes(
        &self,
        id: &str,
        mode_id: Option<String>,
        available_modes: Vec<SessionModeInfo>,
    ) -> Result<()> {
        {
            let mut records = self.records.write().await;
            if let Some(m) = records.get_mut(id) {
                if mode_id.is_some() {
                    m.mode_id = mode_id;
                }
                if !available_modes.is_empty() {
                    m.available_modes = available_modes;
                }
            }
        }
        self.persist().await
    }

    pub async fn set_mode(&self, id: &str, mode_id: String) -> Result<()> {
        {
            let mut records = self.records.write().await;
            if let Some(m) = records.get_mut(id) {
                m.mode_id = Some(mode_id);
                m.updated_at = Utc::now();
            }
        }
        self.persist().await
    }

    pub async fn list(&self) -> Vec<SessionMeta> {
        let mut v: Vec<_> = self.records.read().await.values().cloned().collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        v
    }

    pub async fn get(&self, id: &str) -> Option<SessionMeta> {
        self.records.read().await.get(id).cloned()
    }

    pub async fn archive(&self, id: &str) -> Result<()> {
        self.records.write().await.remove(id);
        self.persist().await
    }

    async fn persist(&self) -> Result<()> {
        let records = self.records.read().await;
        let state = State {
            sessions: records.values().cloned().collect(),
        };
        let data = serde_json::to_vec_pretty(&state)?;
        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.state_path, data).await?;
        Ok(())
    }
}
