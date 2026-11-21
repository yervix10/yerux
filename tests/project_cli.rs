use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "yerux-project-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn new_build_and_run_project() {
    let root = temp();
    let status = Command::new(env!("CARGO_BIN_EXE_yerux"))
        .args(["new", root.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(root.join("yerux.toml").is_file());
    assert!(root.join("src/main.yx").is_file());
    let build = Command::new(env!("CARGO_BIN_EXE_yerux"))
        .current_dir(&root)
        .arg("build")
        .status()
        .unwrap();
    assert!(build.success());
    assert!(root.join("yerux.lock").is_file());
    let run = Command::new(env!("CARGO_BIN_EXE_yerux"))
        .current_dir(&root)
        .arg("run")
        .output()
        .unwrap();
    assert!(run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("Hello from Yerux!"));
    fs::remove_dir_all(root).unwrap();
}
