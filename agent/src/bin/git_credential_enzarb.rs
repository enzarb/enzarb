// Git credential helper: supplies the projected Gitea SA token as the HTTP
// password for the workspace's Gitea host. The gateway's extAuth (authd)
// validates the token and asserts the project identity to Gitea, so the
// username here is arbitrary (never "admin", which authd reserves).
use std::io::{self, BufRead, Write};

const TOKEN_PATH: &str = "/var/run/secrets/enzarb/gitea/token";

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "get" => get(),
        "store" | "erase" => {} // token comes from the projected volume
        _ => {
            eprintln!("usage: git-credential-enzarb <get|store|erase>");
            std::process::exit(1);
        }
    }
}

fn get() {
    // Git feeds key=value lines on stdin (protocol, host, path, ...) ending with
    // a blank line. We only need to echo back username/password.
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) if l.is_empty() => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let token = std::fs::read_to_string(TOKEN_PATH).unwrap_or_else(|e| {
        eprintln!("read Gitea token: {e}");
        std::process::exit(1);
    });

    let mut out = io::stdout();
    let _ = writeln!(out, "username=enzarb");
    let _ = writeln!(out, "password={}", token.trim());
}
