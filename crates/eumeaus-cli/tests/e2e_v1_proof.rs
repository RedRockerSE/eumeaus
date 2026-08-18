//! The v1 proof (SPEC.md §6, milestone M5): drives the real `eumeaus` CLI
//! binary against a real subprocess plugin
//! (`eumeaus-username-search-plugin`) whose HTTP calls are redirected to a
//! local mock server serving recorded cassette fixtures, plus a second,
//! near-instant fixture plugin (`examples/quick_check.rs`) — so a
//! mid-scan kill+resume can show "only incomplete work re-executes"
//! against something that actually finished first. Exercises every
//! module together: engine, plugin host, protocol, persistence,
//! resumability, provenance — the actual point of this milestone.
//!
//! Steps below are numbered to match SPEC.md §6's list exactly.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use assert_cmd::Command as AssertCommand;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn eumeaus_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("eumeaus")
}

/// Same trick as eumeaus-engine's/eumeaus-plugin-host's tests: every
/// workspace crate's compiled artifacts land under one shared
/// `target/<profile>/`, so a test can find another crate's regular
/// `[[bin]]` relative to its own `current_exe()` — there's no
/// `CARGO_BIN_EXE_`-style env var for a bin in a *different* package.
fn workspace_bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(name);
    path
}

/// Same idea, for `[[example]]` targets (own crate's, in this case).
fn workspace_example(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("examples");
    path.push(name);
    path
}

fn write_manifest_text(
    plugin_dir: &Path,
    name: &str,
    entrypoint: &Path,
    signature: Option<&str>,
    default_timeout_ms: u64,
) {
    let signature_line = signature
        .map(|s| format!("signature = \"{s}\"\n"))
        .unwrap_or_default();
    let toml = format!(
        r#"
[plugin]
name = "{name}"
version = "0.1.0"
description = "test fixture"
author = "test"
{signature_line}
[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"
protocol_version = "1"

[contract]
input_entity_types = ["Username"]
output_entity_types = ["OnlineAccount"]
output_relationship_types = ["HasAccount"]

[permissions]
network = true
requested_credentials = []

[execution]
entrypoint = "{entrypoint}"
default_rate_limit_per_sec = 100
default_timeout_ms = {default_timeout_ms}
"#,
        entrypoint = entrypoint.display(),
    );
    std::fs::write(plugin_dir.join("plugin.toml"), toml).unwrap();
}

/// Writes and signs a manifest for real — proving the "signed manifest"
/// M5 deliverable end to end, not just that `--trusted-key` is accepted.
/// The signature covers name+version+binary-hash only, not the entrypoint
/// path (see eumeaus-plugin-host/src/signature.rs), so it's safe to
/// compute it against an unsigned draft and reuse once the real manifest
/// is written with that same signature filled in.
fn write_signed_manifest(
    plugins_dir: &Path,
    name: &str,
    entrypoint: &Path,
    signing_key: &SigningKey,
    default_timeout_ms: u64,
) {
    let plugin_dir = plugins_dir.join(name);
    std::fs::create_dir_all(&plugin_dir).unwrap();

    write_manifest_text(&plugin_dir, name, entrypoint, None, default_timeout_ms);
    let manifest = eumeaus_plugin_host::PluginHost::discover(plugins_dir)
        .unwrap()
        .into_iter()
        .find(|m| m.plugin.name == name)
        .unwrap_or_else(|| panic!("manifest for {name} was just written"));
    let signature = eumeaus_plugin_host::sign(signing_key, &manifest).unwrap();

    write_manifest_text(
        &plugin_dir,
        name,
        entrypoint,
        Some(&signature),
        default_timeout_ms,
    );
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Needs a multi-thread runtime: the test blocks its own thread on real
/// subprocess I/O (`Command::output`/killing a child) while wiremock's
/// mock server must keep answering that subprocess's HTTP requests on a
/// *different* thread at the same time. A current-thread runtime would
/// have nothing left to poll the server with while blocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_proof() {
    let base = tempfile::tempdir().unwrap();
    let plugins_dir = base.path().join("plugins");

    // --- Cassette fixtures (SPEC.md §6 step 2): a known-existing site
    // (github, FOUND), a known-nonexistent one (gitlab, NOT_FOUND), and a
    // rate-limited one (flaky-forum, UNCERTAIN) — with an artificial delay
    // on the last so step 3 below has a real window to kill the process
    // in before it responds.
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/github/carol"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>carol's profile</html>"))
        .mount(&mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gitlab/carol"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky-forum/u/carol"))
        .respond_with(ResponseTemplate::new(429).set_delay(Duration::from_millis(2500)))
        .mount(&mock_server)
        .await;

    let signing_key = SigningKey::generate(&mut OsRng);
    let trusted_key_hex = hex_encode(&signing_key.verifying_key().to_bytes());

    write_signed_manifest(
        &plugins_dir,
        "quick-check",
        &workspace_example("quick_check"),
        &signing_key,
        2_000,
    );
    write_signed_manifest(
        &plugins_dir,
        "username-search",
        &workspace_bin("eumeaus-username-search-plugin"),
        &signing_key,
        8_000,
    );

    // 1. `case create` a fresh encrypted case.
    AssertCommand::new(eumeaus_bin())
        .args(["case", "create", "proof", "--path"])
        .arg(base.path())
        .assert()
        .success();
    let case_path = base.path().join("proof.eum");

    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["entity", "add", "--type", "Username", "--key", "carol"])
        .assert()
        .success();

    // 2. `scan run` against both plugins, with the real HTTP client
    // redirected at the mock server (via env var — see
    // eumeaus-username-search-plugin's crate docs).
    let mut child = Command::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .arg("scan")
        .arg("run")
        .arg("--plugins-dir")
        .arg(&plugins_dir)
        .args([
            "--target-type",
            "Username",
            "--target-value",
            "carol",
            "--trusted-key",
            &trusted_key_hex,
        ])
        .env("EUMEAUS_USERNAME_SEARCH_BASE_URL", mock_server.uri())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn eumeaus scan run");

    let stdout = child.stdout.take().unwrap();
    let scan_id = {
        use std::io::{BufRead, BufReader};
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read scan id from stdout");
        line.trim().to_string()
    };
    assert!(
        !scan_id.is_empty(),
        "expected a scan id on the first stdout line"
    );

    // 3. Kill the process mid-scan (simulated crash). quick-check has
    // certainly finished by now (it's near-instant); flaky-forum's 2.5s
    // mock delay guarantees username-search hasn't.
    std::thread::sleep(Duration::from_millis(400));
    child.kill().expect("kill the scan run process");
    child.wait().expect("reap the killed process");

    // The next case-open (implicit in every command below) reconciles
    // username-search's orphaned RUNNING row back to PENDING (SPEC.md
    // §4.2); this reads it before that reconciliation would otherwise be
    // invisible from the outside.
    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["scan", "status", &scan_id])
        .assert()
        .success()
        .stdout(predicates::str::contains("RUNNING"));

    // `scan resume`: confirm only the incomplete plugin run re-executes —
    // asserted structurally in step 4 below (quick-check's entity must
    // not be duplicated).
    AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .arg("scan")
        .arg("resume")
        .arg(&scan_id)
        .env("EUMEAUS_USERNAME_SEARCH_BASE_URL", mock_server.uri())
        .assert()
        .success()
        // Every plugin invocation itself succeeded (both returned answers
        // within their timeout) — flaky-forum's UNCERTAIN is a property of
        // *that finding*, not of whether username-search's invocation
        // completed, so the scan as a whole is COMPLETED, not
        // PARTIALLY_FAILED (SPEC.md §5).
        .stdout(predicates::str::contains("COMPLETED"));

    // 4. Assert the resulting entity graph matches fixture-derived truth:
    // FOUND sites produced an OnlineAccount + HasAccount, NOT_FOUND/
    // UNCERTAIN ones did not.
    let entities = String::from_utf8(
        AssertCommand::new(eumeaus_bin())
            .arg("--case")
            .arg(&case_path)
            .args(["entity", "list"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();

    assert!(
        entities.contains("carol"),
        "target entity present:\n{entities}"
    );
    assert!(
        entities.contains("github:carol"),
        "github FOUND must produce an entity:\n{entities}"
    );
    assert!(
        entities.contains("quickcheck:carol"),
        "quick-check FOUND must produce an entity, and resume must not \
         have lost it while re-running username-search:\n{entities}"
    );
    assert!(
        !entities.contains("gitlab:carol"),
        "gitlab NOT_FOUND must not produce an entity:\n{entities}"
    );
    assert!(
        !entities.contains("flaky-forum:carol"),
        "flaky-forum UNCERTAIN must not produce an entity:\n{entities}"
    );

    // quick-check's entity must exist as exactly one row — proving resume
    // didn't re-invoke the plugin run that had already succeeded before
    // the kill (counting matching *lines*, not raw substring occurrences,
    // since quick_check.rs's canonical_key and display_label are equal —
    // both would appear on that one row).
    let quickcheck_row_count = entities
        .lines()
        .filter(|line| line.contains("quickcheck:carol"))
        .count();
    assert_eq!(
        quickcheck_row_count, 1,
        "quick-check's entity must appear as exactly one row in `entity list`:\n{entities}"
    );

    // 5. `case close`, then `case open` again; the graph must be
    // unchanged. Every CLI invocation above already opened and closed the
    // case itself (there's no long-running daemon), so this just compares
    // a fresh open's view against the one already captured above.
    let entities_after_reopen = AssertCommand::new(eumeaus_bin())
        .arg("--case")
        .arg(&case_path)
        .args(["entity", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        entities.as_bytes(),
        entities_after_reopen.as_slice(),
        "entity graph must be byte-for-byte identical after a fresh case open"
    );

    // 6. Attempt to open the case file directly with plain sqlite3 and
    // confirm it is unreadable without the key.
    match Command::new("sqlite3")
        .arg(&case_path)
        .arg(".tables")
        .output()
    {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success()
                    || stderr.contains("file is not a database")
                    || stderr.contains("file is encrypted"),
                "plain sqlite3 must not be able to read an encrypted case file"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("sqlite3 binary not found on PATH; skipping unreadability check");
        }
        Err(e) => panic!("failed to run sqlite3: {e}"),
    }
}
