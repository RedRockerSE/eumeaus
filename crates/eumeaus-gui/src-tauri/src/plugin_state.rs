//! G5 (SPEC.md §9.6): plugin list/install/verify — wraps
//! `eumeaus_engine::plugins::discover`/`install`/`verify`, the same calls
//! `eumeaus-cli`'s `plugin list`/`install`/`verify` make. Not
//! case-scoped (a `plugins_dir` is just a path), so unlike
//! `entity_state`/`scan_state` these commands don't touch
//! `case_state::AppState` at all.

use std::path::Path;

use eumeaus_engine::plugins::{self, PluginManifest};
use eumeaus_engine::TrustPolicy;
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct PluginSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub signed: bool,
    pub entrypoint: String,
    pub input_entity_types: Vec<String>,
}

impl From<PluginManifest> for PluginSummary {
    fn from(m: PluginManifest) -> Self {
        PluginSummary {
            name: m.plugin.name.clone(),
            version: m.plugin.version.clone(),
            description: m.plugin.description.clone(),
            signed: m.plugin.signature.is_some(),
            entrypoint: m.entrypoint_path().display().to_string(),
            input_entity_types: m.contract.input_entity_types.clone(),
        }
    }
}

/// Mirrors `eumeaus-cli`'s own `parse_trust_policy`: neither given means
/// `AllowUnsigned`, exactly one means `RequireSignature`. Verifying a
/// plugin specifically (unlike a scan) additionally requires the caller
/// pass one — see `do_plugin_verify`.
fn trust_policy_from(
    trusted_key: Option<&str>,
    trust: Option<&str>,
) -> Result<TrustPolicy, String> {
    match (trusted_key, trust) {
        (Some(hex_key), None) => {
            let bytes = hex_decode(hex_key)?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| "trusted key must be 32 bytes (64 hex chars)".to_string())?;
            let trusted_key = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|e| format!("invalid trusted key: {e}"))?;
            Ok(TrustPolicy::RequireSignature { trusted_key })
        }
        (None, Some(name)) => {
            let trusted_key = eumeaus_engine::trust::resolve(name).map_err(|e| e.to_string())?;
            Ok(TrustPolicy::RequireSignature { trusted_key })
        }
        (None, None) => Ok(TrustPolicy::AllowUnsigned),
        (Some(_), Some(_)) => Err("pass only one of trusted_key or trust, not both".to_string()),
    }
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn do_plugin_list(plugins_dir: &Path) -> Result<Vec<PluginSummary>, String> {
    plugins::discover(plugins_dir)
        .map(|manifests| manifests.into_iter().map(PluginSummary::from).collect())
        .map_err(|e| e.to_string())
}

fn do_plugin_install(source_path: &Path, plugins_dir: &Path) -> Result<PluginSummary, String> {
    plugins::install(source_path, plugins_dir)
        .map(PluginSummary::from)
        .map_err(|e| e.to_string())
}

fn do_plugin_verify(
    name: &str,
    plugins_dir: &Path,
    trusted_key: Option<&str>,
    trust: Option<&str>,
) -> Result<(), String> {
    if trusted_key.is_none() && trust.is_none() {
        return Err("pass a trusted key or a trust-store name to verify against".to_string());
    }
    let manifest = plugins::discover(plugins_dir)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.plugin.name == name)
        .ok_or_else(|| {
            format!(
                "no plugin named {name:?} discovered in {}",
                plugins_dir.display()
            )
        })?;
    let trust_policy = trust_policy_from(trusted_key, trust)?;
    plugins::verify(&manifest, &trust_policy).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn plugin_list(plugins_dir: String) -> Result<Vec<PluginSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || do_plugin_list(Path::new(&plugins_dir)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plugin_install(
    source_path: String,
    plugins_dir: String,
) -> Result<PluginSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        do_plugin_install(Path::new(&source_path), Path::new(&plugins_dir))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plugin_verify(
    name: String,
    plugins_dir: String,
    trusted_key: Option<String>,
    trust: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        do_plugin_verify(
            &name,
            Path::new(&plugins_dir),
            trusted_key.as_deref(),
            trust.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_empty_for_a_directory_with_no_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let found = do_plugin_list(dir.path()).unwrap();
        assert!(found.is_empty());
    }

    fn write_source_manifest(source_dir: &Path, name: &str) {
        std::fs::create_dir_all(source_dir).unwrap();
        std::fs::write(source_dir.join("bin"), b"dummy entrypoint").unwrap();
        std::fs::write(
            source_dir.join("plugin.toml"),
            format!(
                r#"
[plugin]
name = "{name}"
version = "0.1.0"
description = "test fixture"
author = "test"

[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"
protocol_version = "1"

[contract]
input_entity_types = ["Username"]
output_entity_types = []
output_relationship_types = []

[permissions]
network = false
requested_credentials = []

[execution]
entrypoint = "bin"
default_rate_limit_per_sec = 1
default_timeout_ms = 1000
"#
            ),
        )
        .unwrap();
    }

    // SPEC.md §9.6 G5's verify bar in miniature: a plugin installed
    // through do_plugin_install shows up via do_plugin_list with the
    // same data eumeaus-cli's `plugin list` would print (name, version,
    // signed/unsigned, resolved entrypoint path) — the wiring a CLI-
    // installed plugin and a GUI-installed one both go through is
    // identical (plugins::install/discover), so this is the whole proof.
    #[test]
    fn install_then_list_shows_the_installed_plugin() {
        let base = tempfile::tempdir().unwrap();
        let source_dir = base.path().join("source");
        let plugins_dir = base.path().join("plugins");
        write_source_manifest(&source_dir, "fixture-plugin");

        let installed = do_plugin_install(&source_dir, &plugins_dir).unwrap();
        assert_eq!(installed.name, "fixture-plugin");
        assert_eq!(installed.version, "0.1.0");
        assert!(!installed.signed);

        let listed = do_plugin_list(&plugins_dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "fixture-plugin");
        assert!(listed[0].entrypoint.ends_with("bin"));
    }

    #[test]
    fn install_refuses_to_overwrite_an_already_installed_plugin() {
        let base = tempfile::tempdir().unwrap();
        let source_dir = base.path().join("source");
        let plugins_dir = base.path().join("plugins");
        write_source_manifest(&source_dir, "dup-plugin");

        do_plugin_install(&source_dir, &plugins_dir).unwrap();
        let err = do_plugin_install(&source_dir, &plugins_dir).unwrap_err();
        assert!(err.contains("already installed"));
    }

    #[test]
    fn verify_requires_a_trust_key_or_trust_name() {
        let dir = tempfile::tempdir().unwrap();
        let err = do_plugin_verify("anything", dir.path(), None, None).unwrap_err();
        assert!(err.contains("trust"));
    }

    #[test]
    fn verify_errors_when_the_named_plugin_is_not_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let err = do_plugin_verify("nope", dir.path(), Some("00"), None).unwrap_err();
        assert!(err.contains("no plugin named"));
    }
}
