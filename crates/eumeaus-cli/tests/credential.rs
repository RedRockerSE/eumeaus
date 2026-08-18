//! `credential set`'s interactive prompt (SPEC.md §3.4) needs a real TTY —
//! `rpassword` deliberately refuses to read a password from a plain pipe,
//! which isn't reproducible here (or in most CI sandboxes) without a
//! pty-emulation dependency this one command doesn't justify. The
//! underlying store (set/get/list/remove) and its use in credential
//! injection are already covered thoroughly by
//! eumeaus-engine/src/scan.rs's tests, which call it directly. This just
//! covers the two subcommands that don't need a TTY.

use assert_cmd::Command;

fn eumeaus() -> Command {
    Command::cargo_bin("eumeaus").unwrap()
}

#[test]
fn credential_list_runs_without_a_case() {
    // No --case given at all: credentials aren't case-scoped (SPEC.md
    // §4.5), so this must not try to open one.
    eumeaus().args(["credential", "list"]).assert().success();
}

#[test]
fn removing_a_credential_that_was_never_set_is_not_an_error() {
    eumeaus()
        .args(["credential", "remove", "a-name-nobody-ever-set"])
        .assert()
        .success();
}
