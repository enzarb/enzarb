//! Hardened environment for the git calls that carry a token (issue #52).
//!
//! The chat agent (and a job's agent) runs as sandbox-init's uid, so it can
//! write `.git/config` and `.git/hooks` of every clone in the workspace. Git
//! always reads the repository's own config, and `-c`/`GIT_CONFIG_*` cannot
//! remove what it adds (`url.*.insteadOf` still rewrites an explicit URL,
//! `include.path` pulls in more, `http.proxy` or `http.sslCAInfo` reroute
//! TLS). So sandbox-init never runs an authenticated git command inside a
//! clone. Instead each network operation runs in a fresh, empty bare
//! repository that sandbox-init creates for that one call
//! (`<ws>/.enzarb/net/<tag>`), borrowing the clone's objects through
//! `objects/info/alternates`:
//!
//! * fetch: network → net repo, then a local, token-free fetch net repo → clone;
//! * push: net repo (with the worktree's commit reachable through the
//!   alternate) → the explicit URL, `<sha>:refs/heads/<branch>`;
//! * first clone: `git clone` into a directory that does not exist yet.
//!
//! On top of that, every such call gets [`env`]: an empty process
//! environment (no `GIT_CONFIG_PARAMETERS`, `GIT_DIR`, `GIT_SSH*`, askpass,
//! trace or `XDG_CONFIG_HOME` leaking in), no system or global config,
//! `HOME=/nonexistent` (no `.netrc`), and command-line config that disables
//! hooks, fsmonitor, credential helpers, askpass and ssh, and allows only the
//! URL's own scheme (`https`, or `file` in tests). The auth header is passed
//! only as `GIT_CONFIG_*` environment: it is never written to any file.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Attempts per network call (the first try plus two retries).
pub const NET_ATTEMPTS: usize = 3;

/// Pauses between attempts. Three attempts need two delays; the issue's third
/// (90 s) value would never be used.
pub const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(30)];

/// Wall-clock cap of one push attempt: a push sends small packs, so a dead
/// connection (which libcurl's low-speed check does not catch) fails early.
pub const PUSH_ATTEMPT_CAP: Duration = Duration::from_secs(120);

/// Stderr of a failure that will not get better by trying again.
const PERMANENT: &[&str] = &[
    // With their context: a bare "403" also occurs in millisecond counts.
    "error: 401",
    "error: 403",
    "HTTP 401",
    "HTTP 403",
    "Authentication failed",
    "Permission denied",
    "non-fast-forward",
    "[rejected]",
    "[remote rejected]",
    "protected branch",
    "fetch first",
    "not found",
];

/// Stderr of a network failure that is worth retrying.
const TRANSIENT: &[&str] = &[
    "Connection timed out",
    "Connection reset",
    "Could not resolve host",
    "Failed to connect",
    "Operation timed out",
    "RPC failed",
    "early EOF",
    "unexpected disconnect",
    "returned error: 429",
    "The remote end hung up",
    "Operation too slow",
];

/// Whether a failed network git call (its stderr) is a transient network
/// error. Permanent patterns win; unknown errors are not transient.
pub fn is_transient(stderr: &str) -> bool {
    if PERMANENT.iter().any(|p| stderr.contains(p)) {
        return false;
    }
    if TRANSIENT.iter().any(|p| stderr.contains(p)) {
        return true;
    }
    // HTTP 5xx: "The requested URL returned error: 503".
    stderr.match_indices("returned error: 5").any(|(i, m)| {
        stderr[i + m.len()..]
            .chars()
            .take(2)
            .filter(char::is_ascii_digit)
            .count()
            == 2
    })
}

/// Variables passed through from sandbox-init's environment.
const PASS_ENV: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "TZ",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// The protocol git may use for `url`: `https`, or `file` (tests, dev).
pub fn scheme(url: &str) -> Result<&'static str, String> {
    if url.starts_with("https://") {
        Ok("https")
    } else if url.starts_with("file://") {
        Ok("file")
    } else {
        Err("repo_url must be https:// (or file://)".into())
    }
}

/// The whole environment of an authenticated git call to `url`. `base` is
/// the author/committer identity; `header` the `Authorization` header value,
/// if any.
pub fn env(
    url: &str,
    header: Option<&str>,
    base: &[(String, String)],
) -> Result<Vec<(String, String)>, String> {
    let scheme = scheme(url)?;
    let mut env: Vec<(String, String)> = PASS_ENV
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    env.extend(base.iter().cloned());
    for (k, v) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("HOME", "/nonexistent"),
        ("GIT_ASKPASS", ""),
        ("SSH_ASKPASS", ""),
    ] {
        env.retain(|(a, _)| a != k);
        env.push((k.into(), v.into()));
    }
    let allow = format!("protocol.{scheme}.allow");
    let mut cfg: Vec<(String, String)> = [
        ("core.hooksPath", "/dev/null"),
        ("core.fsmonitor", "false"),
        ("core.sshCommand", "false"),
        ("core.askPass", ""),
        // An empty value resets the list of helpers.
        ("credential.helper", ""),
        ("protocol.allow", "never"),
        (allow.as_str(), "always"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
        ("fetch.recurseSubmodules", "false"),
        ("submodule.recurse", "false"),
        // Network keys a raced-in scratch config could set; env config wins.
        ("http.proxy", ""),
        ("http.sslVerify", "true"),
        // No http.sslCert/sslKey pins: git reads an empty value as the path
        // "" and every fetch fails (v0.9.1). A client cert cannot leak the token.
        ("http.cookieFile", ""),
        ("http.saveCookies", "false"),
        ("http.followRedirects", "initial"),
        // Reset, then (maybe) the one header.
        ("http.extraheader", ""),
        // A stalled transfer (under 1 kB/s for 60 s) is aborted, not waited for.
        ("http.lowSpeedLimit", "1000"),
        ("http.lowSpeedTime", "60"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if let Some(h) = header {
        cfg.push(("http.extraheader".into(), h.to_string()));
    }
    env.retain(|(k, _)| !k.starts_with("GIT_CONFIG_"));
    env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    env.push(("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()));
    env.push(("GIT_CONFIG_COUNT".into(), cfg.len().to_string()));
    for (i, (k, v)) in cfg.into_iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), k));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), v));
    }
    Ok(env)
}

/// `args` for a token-free git call of sandbox-init's in a workspace repo:
/// the repo's hooks and fsmonitor (which the agents can plant) do not run.
pub fn local_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec![
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
    ];
    v.extend_from_slice(args);
    v
}

/// The scratch dir for one network operation: `<tag>-<128 random bits>`,
/// so another process cannot pre-create it or find it by name ahead of time.
pub fn net_dir(workspace: &Path, tag: &str) -> std::io::Result<PathBuf> {
    let mut b = [0u8; 16];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut b)?;
    let r: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(workspace
        .join(".enzarb")
        .join("net")
        .join(format!("{tag}-{r}")))
}

/// Removes the scratch repo when dropped.
pub struct NetDir(pub PathBuf);

impl Drop for NetDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Vec<&'a str> {
        env.iter()
            .filter(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn config(env: &[(String, String)]) -> Vec<(String, String)> {
        let n: usize = get(env, "GIT_CONFIG_COUNT")[0].parse().unwrap();
        (0..n)
            .map(|i| {
                (
                    get(env, &format!("GIT_CONFIG_KEY_{i}"))[0].to_string(),
                    get(env, &format!("GIT_CONFIG_VALUE_{i}"))[0].to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn only_the_urls_scheme_is_allowed() {
        assert!(scheme("ssh://git@github.com/a/b").is_err());
        assert!(scheme("git@github.com:a/b").is_err());
        assert!(scheme("http://github.com/a/b").is_err());
        let env = env("https://github.com/a/b", Some("Authorization: x"), &[]).unwrap();
        let cfg = config(&env);
        assert!(cfg.contains(&("protocol.allow".into(), "never".into())));
        assert!(cfg.contains(&("protocol.https.allow".into(), "always".into())));
        assert!(!cfg.iter().any(|(k, _)| k == "protocol.file.allow"));
        assert_eq!(get(&env, "GIT_CONFIG_NOSYSTEM"), vec!["1"]);
        assert_eq!(get(&env, "GIT_CONFIG_GLOBAL"), vec!["/dev/null"]);
        assert_eq!(get(&env, "HOME"), vec!["/nonexistent"]);
        assert!(get(&env, "GIT_CONFIG_PARAMETERS").is_empty());
        // reset first, then the one header, last
        let h: Vec<_> = cfg
            .iter()
            .filter(|(k, _)| k == "http.extraheader")
            .collect();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].1, "");
        assert_eq!(cfg.last().unwrap().1, "Authorization: x");
        assert!(cfg.contains(&("core.hooksPath".into(), "/dev/null".into())));
        assert!(cfg.contains(&("http.lowSpeedLimit".into(), "1000".into())));
        assert!(cfg.contains(&("http.lowSpeedTime".into(), "60".into())));
        assert!(cfg.contains(&("credential.helper".into(), "".into())));
        // An empty path-valued key makes curl read the file "": every fetch fails.
        for k in [
            "http.sslCert",
            "http.sslKey",
            "http.sslCAInfo",
            "http.sslCAPath",
        ] {
            assert!(!cfg.iter().any(|(a, _)| a == k), "{k} must not be pinned");
        }
    }

    #[test]
    fn transient_errors_are_retried() {
        for s in [
            "fatal: unable to access 'https://github.com/a/b/': Connection timed out after 300026 milliseconds",
            "fatal: unable to access 'x': Recv failure: Connection reset by peer",
            "fatal: unable to access 'x': Could not resolve host: github.com",
            "fatal: unable to access 'x': Failed to connect to github.com port 443",
            "fatal: unable to access 'x': Operation timed out after 1000 milliseconds",
            "error: RPC failed; curl 56 GnuTLS recv error",
            "fatal: early EOF",
            "fatal: the remote end hung up unexpectedly\nThe remote end hung up",
            "fatal: unable to access 'x': The requested URL returned error: 502",
            "fatal: unable to access 'x': The requested URL returned error: 503",
            "fatal: unable to access 'x': The requested URL returned error: 429",
            "fatal: unable to access 'x': Operation too slow. Less than 1000 bytes/sec transferred",
        ] {
            assert!(is_transient(s), "{s}");
        }
    }

    #[test]
    fn permanent_and_unknown_errors_are_not_retried() {
        for s in [
            "fatal: unable to access 'x': The requested URL returned error: 401",
            "fatal: unable to access 'x': The requested URL returned error: 403",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'x'",
            "git@github.com: Permission denied (publickey).",
            " ! [rejected]        a -> b (non-fast-forward)",
            " ! [remote rejected] a -> b (protected branch hook declined)",
            "hint: Updates were rejected ... (fetch first)",
            "fatal: repository 'x' not found",
            "The requested URL returned error: 5",
            "fatal: something odd happened",
            "",
        ] {
            assert!(!is_transient(s), "{s}");
        }
        // A permanent pattern beats a transient one.
        assert!(!is_transient(
            "Connection reset\nThe requested URL returned error: 403"
        ));
    }
}
