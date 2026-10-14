use super::*;

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn storage_driver_parsing() {
    assert_eq!(
        storage_driver(&v(&["--storage-driver=overlay2"])).as_deref(),
        Some("overlay2")
    );
    assert_eq!(
        storage_driver(&v(&["--storage-driver", "vfs"])).as_deref(),
        Some("vfs")
    );
    assert_eq!(
        storage_driver(&v(&[
            "--storage-driver=vfs",
            "--x",
            "--storage-driver",
            "overlay2"
        ]))
        .as_deref(),
        Some("overlay2")
    );
    assert_eq!(storage_driver(&v(&["--iptables=false"])), None);
    assert_eq!(storage_driver(&v(&["--storage-driver"])), None);
}

#[test]
fn default_args_select_overlay2() {
    let a: Vec<String> = DEFAULT_ARGS.iter().map(|s| s.to_string()).collect();
    assert_eq!(storage_driver(&a).as_deref(), Some("overlay2"));
}

#[test]
fn reclaim_removes_vfs_and_counts_bytes_leaving_siblings() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    std::fs::create_dir_all(r.join("vfs/a/b")).unwrap();
    std::fs::write(r.join("vfs/a/one"), [0u8; 100]).unwrap();
    std::fs::write(r.join("vfs/a/b/two"), [0u8; 50]).unwrap();
    std::fs::create_dir_all(r.join("overlay2")).unwrap();
    std::fs::create_dir_all(r.join("image")).unwrap();
    assert_eq!(reclaim_vfs(r), Some(150));
    assert!(!r.join("vfs").exists());
    assert!(r.join("overlay2").is_dir() && r.join("image").is_dir());
    // Idempotent.
    assert_eq!(reclaim_vfs(r), None);
}

#[test]
fn reclaim_missing_is_noop() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(reclaim_vfs(t.path()), None);
}

#[test]
fn reclaim_does_not_follow_symlinked_vfs() {
    let t = tempfile::tempdir().unwrap();
    let target = t.path().join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("keep"), b"x").unwrap();
    let root = t.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&target, root.join("vfs")).unwrap();
    assert_eq!(reclaim_vfs(&root), None);
    assert!(target.join("keep").exists());
}
