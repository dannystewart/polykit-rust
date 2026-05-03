//! Behavior tests for polylog init via the init_harness example.

use std::process::Command;

fn run_harness(args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["run", "--quiet", "--example", "init_harness", "--"]);
    cmd.args(args);
    cmd.output().expect("failed to spawn harness")
}

#[test]
fn first_init_succeeds() {
    let out = run_harness(&["first_init"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("first init ok"), "log line missing: {stderr}");
}

#[test]
fn second_init_returns_already_initialized() {
    let out = run_harness(&["second_init_returns_error"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ALREADY_INITIALIZED"), "got: {stdout}");
}

#[test]
fn pre_init_logging_does_not_panic() {
    let out = run_harness(&["pre_init_logging_no_panic"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("PRE_INIT_OK"));
}

#[test]
fn file_init_creates_missing_directory() {
    let tmp = std::env::temp_dir().join(format!("polylog-test-{}", std::process::id()));
    let log = tmp.join("nested/sub/app.log");
    let _ = std::fs::remove_dir_all(&tmp);
    let out = run_harness(&["file_init_creates_dir", log.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let parent = log.parent().unwrap();
    assert!(parent.exists());
    let entries: Vec<_> = std::fs::read_dir(parent).unwrap().flatten().collect();
    assert!(!entries.is_empty(), "no log file created in {parent:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn file_init_unwritable_returns_error() {
    #[cfg(unix)]
    {
        let out = run_harness(&["file_init_unwritable_returns_error"]);
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("FILE_SETUP_FAILED"));
    }
}

#[test]
fn no_color_env_strips_ansi_in_auto_mode() {
    let out = run_harness(&["no_color_env_disables_ansi"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains('\x1b'), "ANSI bytes leaked through NO_COLOR: {stderr:?}");
}

#[test]
fn log_crate_calls_route_through_tracing_log_bridge() {
    let out = run_harness(&["log_crate_bridge"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("from log crate"), "bridge missed: {stderr}");
}
