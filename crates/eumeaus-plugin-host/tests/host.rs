//! End-to-end test defining "done" for milestone M3 (SPEC.md §7):
//!
//!   "a trivial stub plugin is discovered/invoked successfully; a
//!    deliberately-hanging stub plugin is correctly timed out without
//!    crashing the host."
//!
//! Drives the real subprocess/gRPC path against two fixture plugins built
//! from examples/stub_echo.rs and examples/stub_hang.rs (via
//! eumeaus-plugin-sdk, a dev-dependency) — not a mocked transport.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use eumeaus_plugin_host::{
    CheckRequest, PluginError, PluginHost, PluginManifest, RateLimitConfig, TrustPolicy,
};
use rand::rngs::OsRng;

/// `cargo test` compiles examples (unlike plain `cargo build`, which skips
/// them — see Cargo.toml's comment on why these are examples, not
/// `[[bin]]`s) to `<target-dir>/<profile>/examples/<name>`, with no
/// `CARGO_BIN_EXE_`-style env var to find them by (that only covers
/// `[[bin]]` targets). Locate it relative to this test binary's own path
/// instead, which works under any target-dir/profile configuration.
fn example_binary_path(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current test binary path");
    path.pop(); // this test binary's filename
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("examples");
    path.push(name);
    path
}

fn discover_one(plugins_dir: &Path, name: &str) -> PluginManifest {
    PluginHost::discover(plugins_dir)
        .unwrap()
        .into_iter()
        .find(|m| m.plugin.name == name)
        .unwrap_or_else(|| panic!("manifest for {name} was just written to {plugins_dir:?}"))
}

fn write_manifest(
    plugins_dir: &Path,
    name: &str,
    entrypoint: &Path,
    signature: Option<&str>,
) -> PathBuf {
    let plugin_dir = plugins_dir.join(name);
    std::fs::create_dir_all(&plugin_dir).unwrap();

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
network = false
requested_credentials = []

[execution]
entrypoint = "{entrypoint}"
default_rate_limit_per_sec = 5
default_timeout_ms = 2000
"#,
        entrypoint = entrypoint.display(),
    );

    std::fs::write(plugin_dir.join("plugin.toml"), toml).unwrap();
    plugin_dir.join("plugin.toml")
}

/// Loads `manifest_path`, signing it for real against a freshly generated
/// keypair (so the test proves signature *verification*, not just that
/// `AllowUnsigned` bypasses it).
fn signed_manifest(plugins_dir: &Path, name: &str) -> (PluginManifest, SigningKey) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let entrypoint = example_binary_path(name);

    // Sign against an unsigned copy first (signature covers name+version+
    // binary hash, not the signature field itself — see signature.rs).
    write_manifest(plugins_dir, name, &entrypoint, None);
    let unsigned = discover_one(plugins_dir, name);
    let signature = eumeaus_plugin_host::sign(&signing_key, &unsigned).unwrap();

    write_manifest(plugins_dir, name, &entrypoint, Some(&signature));
    let manifest = discover_one(plugins_dir, name);
    (manifest, signing_key)
}

#[tokio::test]
async fn stub_plugin_is_discovered_and_invoked_successfully() {
    let plugins_dir = tempfile::tempdir().unwrap();
    let (manifest, signing_key) = signed_manifest(plugins_dir.path(), "stub_echo");

    let discovered = PluginHost::discover(plugins_dir.path()).unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].plugin.name, "stub_echo");

    let mut host = PluginHost;
    let handle = host
        .load(
            &manifest,
            TrustPolicy::RequireSignature {
                trusted_key: signing_key.verifying_key(),
            },
        )
        .await
        .expect("stub_echo should load and complete its handshake");

    let request = CheckRequest {
        scan_id: "scan-1".to_string(),
        input_entity_type: "Username".to_string(),
        input_value: "carol".to_string(),
        resolved_credentials: Default::default(),
        rate_limit: None,
    };
    let results = host
        .invoke(&handle, request)
        .await
        .expect("stub_echo should respond");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entities.len(), 1);
    assert_eq!(results[0].entities[0].canonical_key, "carol");

    host.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn hanging_plugin_times_out_without_crashing_the_host() {
    let plugins_dir = tempfile::tempdir().unwrap();
    let (manifest, signing_key) = signed_manifest(plugins_dir.path(), "stub_hang");

    let mut host = PluginHost;
    let handle = host
        .load(
            &manifest,
            TrustPolicy::RequireSignature {
                trusted_key: signing_key.verifying_key(),
            },
        )
        .await
        .expect("stub_hang should still complete its handshake");

    let request = CheckRequest {
        scan_id: "scan-1".to_string(),
        input_entity_type: "Username".to_string(),
        input_value: "dave".to_string(),
        resolved_credentials: Default::default(),
        // Override the manifest's 2s default so this test doesn't wait
        // that long for the (expected) timeout.
        rate_limit: Some(RateLimitConfig {
            requests_per_sec: 5,
            timeout_ms: 200,
        }),
    };

    let result = tokio::time::timeout(Duration::from_secs(10), host.invoke(&handle, request))
        .await
        .expect("invoke's own timeout must fire well before this outer safety bound — if it didn't, the host itself is hanging, the exact bug this test guards against");

    assert!(
        matches!(result, Err(PluginError::Timeout(_, _))),
        "expected a Timeout error, got {result:?}"
    );

    // The host process (this test) is still alive and can still make
    // further progress — proving the hang didn't take it down.
    host.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn loading_an_unsigned_plugin_is_refused_by_default() {
    let plugins_dir = tempfile::tempdir().unwrap();
    let entrypoint = example_binary_path("stub_echo");
    write_manifest(plugins_dir.path(), "stub_echo", &entrypoint, None);
    let manifest = discover_one(plugins_dir.path(), "stub_echo");

    let signing_key = SigningKey::generate(&mut OsRng);
    let mut host = PluginHost;
    let err = host
        .load(
            &manifest,
            TrustPolicy::RequireSignature {
                trusted_key: signing_key.verifying_key(),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(err, PluginError::Unsigned(_)));
}

#[tokio::test]
async fn allow_unsigned_bypasses_signature_verification() {
    let plugins_dir = tempfile::tempdir().unwrap();
    let entrypoint = example_binary_path("stub_echo");
    write_manifest(plugins_dir.path(), "stub_echo", &entrypoint, None);
    let manifest = discover_one(plugins_dir.path(), "stub_echo");

    let mut host = PluginHost;
    let handle = host
        .load(&manifest, TrustPolicy::AllowUnsigned)
        .await
        .expect("--allow-unsigned should load an unsigned plugin");
    host.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let plugins_dir = tempfile::tempdir().unwrap();
    let (manifest, _real_signing_key) = signed_manifest(plugins_dir.path(), "stub_echo");

    // A *different* keypair's public key must not accept the signature.
    let wrong_key = SigningKey::generate(&mut OsRng);
    let mut host = PluginHost;
    let err = host
        .load(
            &manifest,
            TrustPolicy::RequireSignature {
                trusted_key: wrong_key.verifying_key(),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(err, PluginError::InvalidSignature(_)));
}
