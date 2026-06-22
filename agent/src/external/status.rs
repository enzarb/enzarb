use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct StatusResponse {
    pub disk: DiskUsage,
}

#[derive(Serialize)]
pub struct DiskUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

pub async fn status() -> Json<StatusResponse> {
    Json(StatusResponse {
        disk: disk_usage("/home/user"),
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
