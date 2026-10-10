//! Compiles actual desktop granted-folder and process modules without Tauri/WebKit.
//! No process invocation is used by the personal source template.
#![allow(dead_code, unused_variables)]
#[path = "../../../apps/desktop/src-tauri/src/desktop_process.rs"]
mod desktop_process;
#[path = "../../../apps/desktop/src-tauri/src/granted_fs.rs"]
mod granted_fs;
use inbharat_harness_core::RootedFs;
#[test]
fn personal_scoped_actual_granted_file_read_denies_outside_and_symlink() {
    let base = tempfile::tempdir().unwrap();
    let granted = base.path().join("granted");
    std::fs::create_dir(&granted).unwrap();
    let file = granted.join("selected.txt");
    std::fs::write(&file, "selected-file-effect-marker").unwrap();
    let outside = base.path().join("secret.txt");
    std::fs::write(&outside, "OUTSIDE-SECRET").unwrap();
    let folders = granted_fs::GrantedFolders::new(vec![RootedFs::new(&granted).unwrap()]).unwrap();
    assert!(folders.try_route_absolute(&outside).is_none());
    assert!(folders.read_text("../secret.txt").is_err());
    let (fence, relative) = folders.try_route_absolute(&file).unwrap();
    assert_eq!(
        fence.read_text(relative).unwrap(),
        "selected-file-effect-marker"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, granted.join("escape.txt")).unwrap();
        assert!(folders.read_text("escape.txt").is_err());
    }
}
