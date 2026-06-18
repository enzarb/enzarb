// Docker credential helper: reads a projected K8s SA token and returns it
// as Bearer credentials for registry.enzarb.dev (Zot TokenReview auth).
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

const TOKEN_PATH: &str = "/var/run/secrets/enzarb/registry/token";
const REGISTRY_HOST: &str = "registry.enzarb.dev";

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "get" => get(),
        "list" => list(),
        "store" | "erase" => {} // tokens come from the projected volume
        _ => {
            eprintln!("usage: docker-credential-k8s-sa <get|list|store|erase>");
            std::process::exit(1);
        }
    }
}

fn get() {
    let stdin = io::stdin();
    let server_url = stdin
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .unwrap_or_default();
    let server_url = server_url.trim().to_string();

    if !server_url.contains(REGISTRY_HOST) {
        eprintln!("not our registry");
        std::process::exit(1);
    }

    let token = std::fs::read_to_string(TOKEN_PATH).unwrap_or_else(|e| {
        eprintln!("read SA token: {e}");
        std::process::exit(1);
    });

    let creds = serde_json::json!({
        "ServerURL": server_url,
        "Username":  "sa-token",
        "Secret":    token.trim(),
    });
    writeln!(io::stdout(), "{}", creds).unwrap_or_else(|e| {
        eprintln!("encode creds: {e}");
        std::process::exit(1);
    });
}

fn list() {
    let mut map = HashMap::new();
    map.insert(REGISTRY_HOST, "sa-token");
    let out = serde_json::to_string(&map).unwrap_or_else(|e| {
        eprintln!("encode list: {e}");
        std::process::exit(1);
    });
    writeln!(io::stdout(), "{out}").unwrap_or_else(|e| {
        eprintln!("write list: {e}");
        std::process::exit(1);
    });
}
