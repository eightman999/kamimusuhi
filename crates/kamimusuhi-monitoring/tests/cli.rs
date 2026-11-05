use std::{fs, process::Command};

#[test]
fn writes_dashboard_and_rejects_extra_arguments_without_overwriting() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("dashboard.json");
    let result = Command::new(env!("CARGO_BIN_EXE_kamimusuhi-monitoring"))
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    let generated = fs::read(&output).unwrap();
    let dashboard: serde_json::Value = serde_json::from_slice(&generated).unwrap();
    assert_eq!(dashboard, kamimusuhi_monitoring::make_dashboard());
    let result = Command::new(env!("CARGO_BIN_EXE_kamimusuhi-monitoring"))
        .arg(&output)
        .arg("unexpected")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("usage:"));
    assert_eq!(fs::read(&output).unwrap(), generated);
}
