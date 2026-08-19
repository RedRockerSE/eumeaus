//! Thin engine-level wrappers around `eumeaus-plugin-host`'s discovery,
//! signature-verification, and install-copy operations, so `eumeaus-cli`'s
//! `plugin list/install/verify` (SPEC.md §3.4) only ever depends on
//! `eumeaus-engine`, same as the rest of the CLI surface — `eumeaus-plugin-host`
//! is not a direct `eumeaus-cli` dependency (see its `Cargo.toml`).

use std::fs;
use std::path::Path;

pub use eumeaus_plugin_host::PluginManifest;

use crate::{EngineError, TrustPolicy};

/// Discovers every plugin manifest in `plugins_dir`. An invalid manifest is
/// skipped with a warning rather than failing the whole listing — same
/// policy as scan orchestration's own discovery (SPEC.md §5).
pub fn discover(plugins_dir: &Path) -> Result<Vec<PluginManifest>, EngineError> {
    Ok(eumeaus_plugin_host::discover(plugins_dir)?)
}

/// Checks `manifest` is compatible with this engine build and, per
/// `trust_policy`, that its signature verifies. Compatibility is checked
/// too (not just the signature) because a plugin that verifies but can't
/// actually be loaded isn't meaningfully "verified".
pub fn verify(manifest: &PluginManifest, trust_policy: &TrustPolicy) -> Result<(), EngineError> {
    eumeaus_plugin_host::check_compatibility(manifest)?;
    eumeaus_plugin_host::verify(manifest, trust_policy)?;
    Ok(())
}

/// Copies `source_dir` (a directory containing `plugin.toml` and its
/// entrypoint binary) into `<plugins_dir>/<plugin-name>/`. Refuses to
/// overwrite an already-installed plugin of the same name — remove it by
/// hand first.
pub fn install(source_dir: &Path, plugins_dir: &Path) -> Result<PluginManifest, EngineError> {
    let manifest = eumeaus_plugin_host::load_file(&source_dir.join("plugin.toml"))?;

    let dest_dir = plugins_dir.join(&manifest.plugin.name);
    if dest_dir.exists() {
        return Err(EngineError::PluginAlreadyInstalled(
            manifest.plugin.name.clone(),
        ));
    }
    copy_dir_recursive(source_dir, &dest_dir)?;
    Ok(manifest)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::path::PathBuf;

    /// Writes `<plugins_dir>/<name>/plugin.toml` plus a dummy entrypoint
    /// file (its *content* matters — the signature covers the entrypoint
    /// binary's sha256, see eumeaus-plugin-host's signature.rs).
    fn write_manifest(plugins_dir: &Path, name: &str, signature: Option<&str>) -> PathBuf {
        let plugin_dir = plugins_dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("bin"), b"dummy entrypoint contents").unwrap();

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
entrypoint = "bin"
default_rate_limit_per_sec = 5
default_timeout_ms = 2000
"#,
        );
        fs::write(plugin_dir.join("plugin.toml"), toml).unwrap();
        plugin_dir
    }

    #[test]
    fn discover_finds_every_manifest_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "a", None);
        write_manifest(dir.path(), "b", None);

        let found = discover(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn verify_accepts_a_correctly_signed_manifest_and_rejects_the_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "signed", None);
        let unsigned = discover(dir.path()).unwrap().remove(0);

        let signing_key = SigningKey::generate(&mut OsRng);
        let signature = eumeaus_plugin_host::sign(&signing_key, &unsigned).unwrap();
        write_manifest(dir.path(), "signed", Some(&signature));
        let manifest = discover(dir.path()).unwrap().remove(0);

        verify(
            &manifest,
            &TrustPolicy::RequireSignature {
                trusted_key: signing_key.verifying_key(),
            },
        )
        .expect("correctly signed manifest should verify");

        let wrong_key = SigningKey::generate(&mut OsRng);
        let err = verify(
            &manifest,
            &TrustPolicy::RequireSignature {
                trusted_key: wrong_key.verifying_key(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::PluginHost(_)));
    }

    #[test]
    fn verify_allow_unsigned_accepts_a_plugin_with_no_signature() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "unsigned", None);
        let manifest = discover(dir.path()).unwrap().remove(0);

        verify(&manifest, &TrustPolicy::AllowUnsigned).unwrap();
    }

    #[test]
    fn install_copies_the_plugin_directory_and_refuses_a_second_install() {
        let source = tempfile::tempdir().unwrap();
        let plugin_dir = write_manifest(source.path(), "copy-me", None);
        let plugins_dir = tempfile::tempdir().unwrap();

        let manifest = install(&plugin_dir, plugins_dir.path()).unwrap();
        assert_eq!(manifest.plugin.name, "copy-me");
        assert!(plugins_dir.path().join("copy-me/plugin.toml").exists());
        assert!(plugins_dir.path().join("copy-me/bin").exists());

        let err = install(&plugin_dir, plugins_dir.path()).unwrap_err();
        assert!(matches!(err, EngineError::PluginAlreadyInstalled(_)));
    }
}
