//! Helpers shared by the integration tests.
#![allow(dead_code)] // each test binary uses a subset

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Restores a directory's mode on drop, so a panic cannot leave it read-only.
struct Restore<'a> {
    dir: &'a Path,
    mode: u32,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.dir, std::fs::Permissions::from_mode(self.mode));
    }
}

/// Run `f` while nothing new can be created in `dir` (like a full disk: the directory is mode
/// 0o555). The previous mode is restored afterwards, also when `f` panics.
///
/// Permissions do not bind root, so this panics instead of letting a test pass vacuously when
/// creation still works.
pub fn read_only<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o7777;
    let _restore = Restore { dir, mode };
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join(".read-only-probe");
    if std::fs::File::create(&probe).is_ok() {
        let _ = std::fs::remove_file(&probe);
        panic!("read_only fixture needs a non-root user: files can still be created in {dir:?}");
    }
    f()
}
