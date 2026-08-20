//! Plugin manifest (`plugin.toml`, SPEC.md §3.3) parsing, discovery, and
//! engine-compatibility checks.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::PluginError;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginSection,
    pub compatibility: CompatibilitySection,
    #[serde(default)]
    pub contract: ContractSection,
    #[serde(default)]
    pub permissions: PermissionsSection,
    pub execution: ExecutionSection,

    /// Directory containing `plugin.toml`; `execution.entrypoint` is
    /// resolved relative to this. Populated by [`discover`]/[`load_file`],
    /// not part of the TOML itself.
    #[serde(skip)]
    pub manifest_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginSection {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// Base64 detached signature. `None` — whether the field is absent
    /// from the TOML, or present but empty (`signature = ""`, the
    /// convention this project's own shipped reference manifests use as a
    /// visible "not signed yet" placeholder, e.g.
    /// `eumeaus-username-search-plugin/plugin.toml`) — means unsigned,
    /// refused to load unless [`crate::TrustPolicy::AllowUnsigned`]. A
    /// real signature is never empty, so normalizing both spellings of
    /// "nothing here" to `None` keeps every caller (the `signed`/
    /// `unsigned` label in `plugin list`, [`crate::signature::verify`])
    /// from having to know about the empty-string convention itself.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub signature: Option<String>,
}

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    Ok(value.filter(|s| !s.is_empty()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompatibilitySection {
    pub engine_min: String,
    /// Either a full semver ("0.2.0") or a major-version wildcard ("0.x").
    pub engine_max: String,
    pub protocol_version: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContractSection {
    #[serde(default)]
    pub input_entity_types: Vec<String>,
    #[serde(default)]
    pub output_entity_types: Vec<String>,
    #[serde(default)]
    pub output_relationship_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PermissionsSection {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub requested_credentials: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionSection {
    pub entrypoint: PathBuf,
    pub default_rate_limit_per_sec: u32,
    pub default_timeout_ms: u64,
}

impl PluginManifest {
    pub fn entrypoint_path(&self) -> PathBuf {
        self.manifest_dir.join(&self.execution.entrypoint)
    }
}

/// Scans `plugins_dir` for `<plugin-name>/plugin.toml` manifests. Per
/// SPEC.md §5 ("other valid plugins still load normally"), an invalid
/// manifest is skipped with a warning rather than aborting discovery of
/// the rest.
pub fn discover(plugins_dir: &Path) -> Result<Vec<PluginManifest>, PluginError> {
    let entries = match std::fs::read_dir(plugins_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(PluginError::Io(e)),
    };

    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("plugin.toml");
        if !manifest_path.exists() {
            continue;
        }
        match load_file(&manifest_path) {
            Ok(manifest) => manifests.push(manifest),
            Err(e) => eprintln!(
                "warning: skipping plugin at {}: {e}",
                manifest_path.display()
            ),
        }
    }
    Ok(manifests)
}

pub fn load_file(manifest_path: &Path) -> Result<PluginManifest, PluginError> {
    let text = std::fs::read_to_string(manifest_path)?;
    let mut manifest: PluginManifest = toml::from_str(&text)
        .map_err(|e| PluginError::InvalidManifest(manifest_path.to_path_buf(), e.to_string()))?;
    manifest.manifest_dir = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok(manifest)
}

/// Hardcoded to "1" for now — matches `plugin.proto`'s only defined
/// version. Will need to become a real negotiated set once the protocol
/// actually changes.
const HOST_PROTOCOL_VERSION: &str = "1";

/// Refuses a manifest that declares itself incompatible with this host
/// build, per SPEC.md §5 ("Plugin manifest invalid or engine-incompatible
/// ... refused at discovery/load time with a clear message").
///
/// `engine_min`/`engine_max` are checked against this crate's own package
/// version, standing in for "the engine's version" — there is no separately
/// versioned engine release yet (every workspace crate shares one
/// `[workspace.package] version`).
pub fn check_compatibility(manifest: &PluginManifest) -> Result<(), PluginError> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("workspace package version is valid semver");

    let min_ok = semver::Version::parse(&manifest.compatibility.engine_min)
        .map(|min| current >= min)
        .unwrap_or(false);
    if !min_ok {
        return Err(PluginError::Incompatible(
            manifest.plugin.name.clone(),
            format!(
                "requires engine >= {}, running {current}",
                manifest.compatibility.engine_min
            ),
        ));
    }

    if !engine_satisfies_max(&current, &manifest.compatibility.engine_max) {
        return Err(PluginError::Incompatible(
            manifest.plugin.name.clone(),
            format!(
                "requires engine <= {}, running {current}",
                manifest.compatibility.engine_max
            ),
        ));
    }

    if manifest.compatibility.protocol_version != HOST_PROTOCOL_VERSION {
        return Err(PluginError::Incompatible(
            manifest.plugin.name.clone(),
            format!(
                "speaks protocol {}, host speaks {HOST_PROTOCOL_VERSION}",
                manifest.compatibility.protocol_version
            ),
        ));
    }

    Ok(())
}

/// `engine_max` per SPEC.md §3.3's example is `"0.x"` — a major-version
/// wildcard, not valid semver on its own — alongside a plain version being
/// just as plausible. Support both.
fn engine_satisfies_max(current: &semver::Version, max: &str) -> bool {
    if let Some(major_str) = max.strip_suffix(".x") {
        return major_str
            .parse::<u64>()
            .map(|major| current.major == major)
            .unwrap_or(false);
    }
    semver::Version::parse(max)
        .map(|max| current <= &max)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_toml(signature_line: &str) -> String {
        format!(
            r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
{signature_line}

[compatibility]
engine_min = "0.1.0"
engine_max = "0.x"
protocol_version = "1"

[execution]
entrypoint = "./bin"
default_rate_limit_per_sec = 5
default_timeout_ms = 8000
"#
        )
    }

    #[test]
    fn a_signature_field_absent_entirely_parses_as_unsigned() {
        let manifest: PluginManifest = toml::from_str(&manifest_toml("")).unwrap();
        assert_eq!(manifest.plugin.signature, None);
    }

    #[test]
    fn an_empty_signature_string_also_parses_as_unsigned() {
        // The convention this project's own shipped reference manifests
        // use as a visible "not signed yet" placeholder (see
        // eumeaus-username-search-plugin/plugin.toml) — must behave
        // identically to the field being absent, not report as signed.
        let manifest: PluginManifest = toml::from_str(&manifest_toml(r#"signature = """#)).unwrap();
        assert_eq!(manifest.plugin.signature, None);
    }

    #[test]
    fn a_real_signature_string_is_preserved() {
        let manifest: PluginManifest =
            toml::from_str(&manifest_toml(r#"signature = "abc123""#)).unwrap();
        assert_eq!(manifest.plugin.signature, Some("abc123".to_string()));
    }
}
