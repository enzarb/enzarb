use axum::{Json, extract::State};
use serde::Serialize;

use crate::AppState;
use crate::init::home_dir;

#[derive(Serialize)]
pub struct StatusResponse {
    pub disk: DiskUsage,
    pub home_dir: String,
    pub project_dir: Option<String>,
}

#[derive(Serialize)]
pub struct DiskUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

pub async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    let home = home_dir();
    let home_str = home.to_string_lossy().into_owned();

    // The repo is cloned to $HOME/<project-slug> by init::setup_git.
    let project_dir = {
        let candidate = home.join(&state.project_slug);
        if candidate.is_dir() {
            Some(candidate.to_string_lossy().into_owned())
        } else {
            None
        }
    };

    Json(StatusResponse {
        disk: disk_usage(&home_str),
        home_dir: home_str,
        project_dir,
    })
}

fn disk_usage(path: &str) -> DiskUsage {
    match nix::sys::statvfs::statvfs(path) {
        Ok(s) => {
            let block = s.fragment_size();
            let total = s.blocks() * block;
            let avail = s.blocks_available() * block;
            DiskUsage {
                used_bytes: total.saturating_sub(avail),
                total_bytes: total,
            }
        }
        Err(_) => DiskUsage {
            used_bytes: 0,
            total_bytes: 0,
        },
    }
}
