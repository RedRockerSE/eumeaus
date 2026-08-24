//! End-to-end coverage for `entity hide`/`entity unhide` (issue #9):
//! dismissing a false-positive finding (e.g. from a large `sites.toml`
//! scan) without deleting anything. Drives the real `eumeaus` binary
//! against a real case file, per SPEC.md §6's black-box testing approach.
//!
//! Requires a running, unlocked OS Secret Service — see CLAUDE.md
//! "Gotchas" (same requirement as every other test that touches
//! `case create`/`case open`).

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;

fn eumeaus_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("eumeaus")
}

fn stdout_of(cmd: &mut AssertCommand) -> String {
    String::from_utf8(cmd.assert().success().get_output().stdout.clone()).unwrap()
}

#[test]
fn hide_excludes_from_list_by_default_and_unhide_reverses_it() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let case_path = tmp.path().join("hide-test.eum");

    AssertCommand::new(eumeaus_bin())
        .args(["case", "create", "hide-test", "--path"])
        .arg(tmp.path())
        .assert()
        .success();

    let entity_id = stdout_of(
        AssertCommand::new(eumeaus_bin())
            .arg("--case")
            .arg(&case_path)
            .args([
                "entity",
                "add",
                "--type",
                "OnlineAccount",
                "--key",
                "false-positive",
            ]),
    )
    .trim()
    .to_string();
    assert!(!entity_id.is_empty());

    // Visible before hiding.
    let before = stdout_of(
        AssertCommand::new(eumeaus_bin())
            .arg("--case")
            .arg(&case_path)
            .args(["entity", "list"]),
    );
    assert!(before.contains("false-positive"), "before hide:\n{before}");

    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args([
            "entity",
            "hide",
            &entity_id,
            "--reason",
            "not actually them",
        ])
        .assert()
        .success();

    // Excluded from `entity list` by default.
    let after_hide = stdout_of(
        AssertCommand::new(eumeaus_bin())
            .arg("--case")
            .arg(&case_path)
            .args(["entity", "list"]),
    );
    assert!(
        !after_hide.contains("false-positive"),
        "hidden entity must be excluded by default:\n{after_hide}"
    );

    // Present again with --include-hidden, and marked as hidden.
    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["entity", "list", "--include-hidden"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("false-positive").and(predicate::str::contains("(hidden)")),
        );

    // Audit trail records the hide.
    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["audit", "show", "--entity", &entity_id])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("hide").and(predicate::str::contains("not actually them")),
        );

    // `entity unhide` reverses it.
    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["entity", "unhide", &entity_id])
        .assert()
        .success();

    let after_unhide = stdout_of(
        AssertCommand::new(eumeaus_bin())
            .arg("--case")
            .arg(&case_path)
            .args(["entity", "list"]),
    );
    assert!(
        after_unhide.contains("false-positive"),
        "unhidden entity must be visible again:\n{after_unhide}"
    );
}
