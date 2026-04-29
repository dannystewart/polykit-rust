use std::process::Command;

fn run_harness(args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["run", "--quiet", "--example", "init_harness", "--"]);
    cmd.args(args);
    cmd.output().expect("failed to spawn harness")
}

#[test]
fn level_override_changes_visibility_in_scope() {
    let out = run_harness(&["level_override_in_scope"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("now visible"),
        "override didn't enable debug: {stderr}"
    );
    assert!(
        !stderr.contains("filtered again"),
        "debug logged after override dropped: {stderr}"
    );
}

#[test]
fn catch_logs_error_chain_to_stderr() {
    let out = run_harness(&["catch_logs_error_chain"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ctx:"), "missing context: {stderr}");
    assert!(stderr.contains("caused by:"), "missing caused by: {stderr}");
}

#[test]
fn concurrent_logging_does_not_corrupt_lines() {
    let out = run_harness(&["concurrent_logging_no_corruption"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line_count = stderr
        .lines()
        .filter(|l| l.contains("[INFO]") && l.contains("CONCURRENT_MARKER"))
        .count();
    assert_eq!(
        line_count, 800,
        "expected 800 lines, got {line_count}: {stderr}"
    );
}
