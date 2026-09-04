//! End-to-end test defining "done" for milestone M1 (SPEC.md §7):
//!
//!   "Create/open/close a case; core schema migrations in place.
//!    Verify: case create/case open round-trip; case file is confirmed
//!    unreadable by plain sqlite3 without the OS-keychain key."
//!
//! Drives the actual `eumeaus` binary, not internal engine APIs, per
//! SPEC.md §6. `Case::create`/`Case::open` are implemented (M1 is done), so
//! this now passes; it was the acceptance test for M1's "done" and stays as
//! a regression test.
//!
//! Requires a running, unlocked OS Secret Service (see `CLAUDE.md`
//! "Gotchas") since `case create`/`case open` touch the real OS keychain.
//!
//! Assumed convention (not yet specified elsewhere): `case create <name>
//! --path <dir>` creates `<dir>/<name>.eum`.

use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;

// Known cost: each run leaves an orphaned key in the real OS keychain
// (service "eumeaus", entry = a fresh random case UUID). This test drives
// the CLI as a black box per SPEC.md §6, and there's no CLI surface yet to
// clean that up (`credential remove` is M6) — harmless, but see CLAUDE.md.
#[test]
fn case_create_open_round_trip_and_encryption_at_rest() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let case_name = "test-case";
    let case_path = tmp.path().join(format!("{case_name}.eum"));

    // 1. `case create` a fresh encrypted case.
    AssertCommand::cargo_bin("eumeaus")
        .expect("find eumeaus binary")
        .args(["case", "create", case_name, "--path"])
        .arg(tmp.path())
        .assert()
        .success();

    assert!(
        case_path.exists(),
        "expected case file at {case_path:?} after `case create`"
    );

    // 2. `case open` the freshly created case: round-trip must succeed.
    AssertCommand::cargo_bin("eumeaus")
        .expect("find eumeaus binary")
        .arg("case")
        .arg("open")
        .arg(&case_path)
        .assert()
        .success();

    // 3. Prove encryption-at-rest is real, not decorative: plain `sqlite3`
    // must NOT be able to read the case file without the OS-keychain key.
    //
    // A real SQL query is used rather than a dot-command (e.g. `.tables`):
    // on some sqlite3 CLI builds (observed: 3.53.4 on Arch Linux),
    // `.tables` against an encrypted file exits 0 with no output instead
    // of surfacing the underlying read error, while a real query reliably
    // reports it (see GitHub issue #11).
    let sqlite3_result = Command::new("sqlite3")
        .arg(&case_path)
        .arg("SELECT * FROM sqlite_master;")
        .output();

    match sqlite3_result {
        Ok(output) => {
            assert!(
                !output.status.success()
                    || predicate::str::contains("file is not a database")
                        .or(predicate::str::contains("file is encrypted"))
                        .eval(&String::from_utf8_lossy(&output.stderr)),
                "plain sqlite3 should not be able to read an encrypted case file"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("sqlite3 binary not found on PATH; skipping unreadability check");
        }
        Err(e) => panic!("failed to run sqlite3: {e}"),
    }
}
