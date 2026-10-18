//! Two worktrees building into one `CARGO_TARGET_DIR`, then two simultaneous builds: all must
//! succeed (a build waits for Cargo's build-dir lock instead of failing).
//! Skipped when `cargo` is not on `PATH`.

use std::path::Path;
use std::process::{Command, Output};

fn cargo(dir: &Path, target: &Path) -> Option<Output> {
    Command::new("cargo")
        .args(["build", "--offline", "-v"])
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTC_WRAPPER")
        .output()
        .ok()
}

fn krate(root: &Path, name: &str, main: &str) -> std::path::PathBuf {
    let d = root.join(name);
    std::fs::create_dir_all(d.join("dep/src")).unwrap();
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(
        d.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ndep = { path = \"dep\" }\n",
    )
    .unwrap();
    std::fs::write(
        d.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(d.join("dep/src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(d.join("src/main.rs"), main).unwrap();
    d
}

#[test]
fn worktrees_share_a_target_dir_and_builds_wait_for_the_lock() {
    let t = tempfile::tempdir().unwrap();
    let target = t.path().join("target");
    let a = krate(t.path(), "a", "fn main() { dep::f(); }\n");
    let b = krate(t.path(), "b", "fn main() { dep::f(); println!(\"b\"); }\n");
    let Some(first) = cargo(&a, &target) else {
        return;
    };
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = cargo(&b, &target).unwrap();
    let err = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{err}");
    assert!(target.join("debug").is_dir());
    assert!(target.join("debug").is_dir());

    // Two at once: both exit 0.
    std::fs::write(
        a.join("src/main.rs"),
        "fn main() { dep::f(); println!(\"x\"); }\n",
    )
    .unwrap();
    std::fs::write(
        b.join("src/main.rs"),
        "fn main() { dep::f(); println!(\"y\"); }\n",
    )
    .unwrap();
    let (t1, t2) = (target.clone(), target.clone());
    let (a2, b2) = (a.clone(), b.clone());
    let h1 = std::thread::spawn(move || cargo(&a2, &t1).unwrap());
    let h2 = std::thread::spawn(move || cargo(&b2, &t2).unwrap());
    for o in [h1.join().unwrap(), h2.join().unwrap()] {
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
}
