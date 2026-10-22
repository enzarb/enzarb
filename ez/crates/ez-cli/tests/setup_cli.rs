//! `ez setup --non-interactive` and `ez keys` against the built binary, in temp dirs.

use std::io::Write;
use std::process::{Command, Output, Stdio};

struct Env {
    proj: tempfile::TempDir,
    cfg: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            proj: tempfile::tempdir().unwrap(),
            cfg: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn rc(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ez"));
        c.args(args)
            .current_dir(self.proj.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg"))
            .env("EZ_CONFIG_DIR", self.cfg.path())
            .env_remove("EZ_PROFILE")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.unwrap_or("").as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.cfg.path().join(rel)).unwrap()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const MODES: &str = "default_mode = \"explore\"\n# team file\n[modes.explore]\ndescription = \"x\"\n[modes.test]\ndescription = \"y\"\nmodel = \"old\"\n";

#[test]
fn non_interactive_writes_config_and_modes() {
    let e = Env::new();
    std::fs::create_dir_all(e.proj.path().join(".ez")).unwrap();
    std::fs::write(e.proj.path().join(".ez/modes.toml"), MODES).unwrap();
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "anthropic",
            "--api-key-stdin",
            "--models",
            "claude-opus-5-5,claude-haiku-5-5",
            "--mode-model",
            "explore=claude-haiku-5-5",
            "--mode-model",
            "test=default",
        ],
        Some("sk-ant-secret-1\n"),
    );
    assert!(o.status.success(), "{}", text(&o));
    let cfg = e.read("config.toml");
    assert!(cfg.contains("[profiles.anthropic]"), "{cfg}");
    assert!(cfg.contains("api_key_name = \"anthropic\""), "{cfg}");
    assert!(
        !cfg.contains("api_key_env") && !cfg.contains("sk-ant"),
        "{cfg}"
    );
    let creds = e.read("credentials.toml");
    assert!(creds.contains("sk-ant-secret-1"), "{creds}");
    assert!(cfg.contains("default_profile = \"anthropic\""), "{cfg}");
    assert!(cfg.contains("\"claude-haiku-5-5\""), "{cfg}");
    let modes = std::fs::read_to_string(e.proj.path().join(".ez/modes.toml")).unwrap();
    assert!(modes.contains("# team file"), "{modes}");
    assert!(modes.contains("profile = \"anthropic\""), "{modes}");
    assert!(modes.contains("model = \"claude-haiku-5-5\""), "{modes}");
    assert!(!modes.contains("old"), "{modes}");
    // Setup is idempotent and the result is usable config.
    let o = e.rc(&["sessions"], None);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn non_interactive_errors_are_clear() {
    let e = Env::new();
    let o = e.rc(&["setup", "--non-interactive", "--models", "a"], None);
    assert!(
        !o.status.success() && text(&o).contains("--provider"),
        "{}",
        text(&o)
    );
    let o = e.rc(
        &["setup", "--non-interactive", "--provider", "openai"],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("--models"),
        "{}",
        text(&o)
    );
    // OpenAI-compatible needs a URL; remote endpoints need a key; the URL must be http(s).
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "openai",
            "--models",
            "a",
        ],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("--base-url"),
        "{}",
        text(&o)
    );
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "openai",
            "--models",
            "a",
            "--base-url",
            "localhost:1",
        ],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("http"),
        "{}",
        text(&o)
    );
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "openai",
            "--models",
            "a",
            "--base-url",
            "https://api.openai.com/v1",
        ],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("API key is required"),
        "{}",
        text(&o)
    );
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "ollama",
            "--models",
            "a",
        ],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("anthropic or openai"),
        "{}",
        text(&o)
    );
    assert!(!e.cfg.path().join("config.toml").exists());
    // Pinning a mode model without modes.toml fails and writes nothing.
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "openai",
            "--base-url",
            "http://localhost:11434/v1",
            "--models",
            "llama3",
            "--mode-model",
            "explore=llama3",
        ],
        None,
    );
    assert!(
        !o.status.success() && text(&o).contains("ez init"),
        "{}",
        text(&o)
    );
    assert!(!e.cfg.path().join("config.toml").exists());
}

#[cfg(unix)]
#[test]
fn named_keys_end_to_end_never_print_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let secret = "sk-ant-SUPERSECRET-wxyz";
    let o = e.rc(
        &[
            "setup",
            "--non-interactive",
            "--provider",
            "anthropic",
            "--models",
            "claude-opus-5-5",
            "--api-key-name",
            "work",
            "--api-key-stdin",
        ],
        Some(&format!("{secret}\n")),
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(!text(&o).contains("SUPERSECRET"));
    let cred = e.cfg.path().join("credentials.toml");
    assert_eq!(
        std::fs::metadata(&cred).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(e.read("credentials.toml").contains(secret));
    assert!(!e.read("config.toml").contains("SUPERSECRET"));
    assert!(e.read("config.toml").contains("api_key_name = \"work\""));

    // keys add (stdin) + list
    let o = e.rc(
        &["keys", "add", "client-acme", "--description", "Acme"],
        Some("sk-acme-1111-abcd\n"),
    );
    assert!(o.status.success(), "{}", text(&o));
    let o = e.rc(&["keys", "list"], None);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("work") && out.contains("…wxyz"), "{out}");
    assert!(
        out.contains("client-acme") && out.contains("…abcd") && out.contains("Acme"),
        "{out}"
    );
    assert!(out.contains("profile `anthropic` (user)"), "{out}");
    assert!(
        !out.contains("SUPERSECRET") && !out.contains("sk-acme-1111"),
        "{out}"
    );

    // keys use --project writes only a reference to the project config.
    let o = e.rc(
        &[
            "keys",
            "use",
            "client-acme",
            "--profile",
            "anthropic",
            "--project",
        ],
        None,
    );
    assert!(o.status.success(), "{}", text(&o));
    let pc = std::fs::read_to_string(e.proj.path().join(".ez/config.toml")).unwrap();
    assert!(
        pc.contains("api_key_name = \"client-acme\"") && !pc.contains("sk-"),
        "{pc}"
    );
    let out = text(&e.rc(&["keys", "list"], None));
    assert!(out.contains("profile `anthropic` (this project)"), "{out}");

    // Unknown keys are rejected; removal works and warns about references.
    let o = e.rc(&["keys", "use", "nope"], None);
    assert!(!o.status.success());
    let o = e.rc(&["keys", "remove", "client-acme"], None);
    assert!(
        o.status.success() && text(&o).contains("still referenced"),
        "{}",
        text(&o)
    );
    assert!(!e.read("credentials.toml").contains("client-acme"));
    let out = text(&e.rc(&["keys", "list"], None));
    assert!(
        out.contains("references missing key `client-acme`"),
        "{out}"
    );
    let o = e.rc(&["keys", "remove", "client-acme"], None);
    assert!(!o.status.success());
}

#[test]
fn headless_without_a_key_suggests_setup() {
    let e = Env::new();
    let o = e.rc(&["-p", "hello"], None);
    assert!(!o.status.success());
    assert!(text(&o).contains("ez setup"), "{}", text(&o));
    // A named key that is missing says how to add it.
    let o = e.rc(&["keys", "use", "ghost"], None);
    assert!(!o.status.success());
    std::fs::write(
        e.cfg.path().join("config.toml"),
        "[profiles.anthropic]\napi_key_name = \"ghost\"\n",
    )
    .unwrap();
    let o = e.rc(&["-p", "hello"], None);
    assert!(text(&o).contains("ez keys add ghost"), "{}", text(&o));
}
