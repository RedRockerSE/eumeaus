//! `eumeaus-email-lookup-plugin` — the second real plugin (following
//! `eumeaus-username-search-plugin`, SPEC.md §7 M5): checks whether an
//! email address has a registered avatar profile on Gravatar and/or
//! Libravatar (a federated, open alternative that intentionally mirrors
//! Gravatar's own API), without needing an API key.
//!
//! Both services key an avatar lookup on an MD5 hash of the (trimmed,
//! lowercased) email address — their own documented contract, not this
//! project's choice — and answer `?d=404` on the avatar URL with a plain
//! HTTP 404 when no avatar is registered, or the image (200) when one is.
//! That's the entire detection strategy: unlike `username-search`'s sites,
//! which need both a pure-status-code and a body-content-marker strategy
//! to reflect how real "is this taken" checks vary, both of this plugin's
//! providers share one identical, stable API shape, so there's no need
//! for `username-search`'s `Detection` enum here.
//!
//! Provider list is externalized the same way `username-search`
//! externalizes its site list: see [`load_providers`] and its module doc
//! for the override/discovery/fallback order. That directory is told to
//! the plugin process via `EUMEAUS_PLUGIN_MANIFEST_DIR` (set by
//! `eumeaus-plugin-host`, see `host.rs`'s module doc).
//!
//! **Trust boundary note:** the plugin's Ed25519 signature (SPEC.md §3.3)
//! covers only `name + version + entrypoint-binary-hash` (see
//! `eumeaus-plugin-host/src/signature.rs`) — never `providers.toml`.
//! Editing that file to add/remove a provider therefore does *not*
//! invalidate a plugin's signature, same caveat as `username-search`'s
//! `sites.toml`.
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_EMAIL_LOOKUP_BASE_URL` redirects every provider's
//! requests to a local mock server instead of the real domain, without
//! changing any of the request/response handling code. See `tests/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// Used both as the manifest-facing identifier and, in mock-server
    /// tests, as the path segment that disambiguates this provider on the
    /// one shared mock server (real providers each have their own domain;
    /// the override doesn't).
    pub slug: String,
    pub display_name: String,
    pub base_url: String,
    /// A path containing the literal substring `{hash}`, replaced verbatim
    /// with the email's MD5 hex digest — e.g. `/avatar/{hash}?d=404`.
    pub path_template: String,
}

fn render_path(template: &str, hash: &str) -> String {
    template.replace("{hash}", hash)
}

/// The built-in provider list: used whenever no `providers.toml` is found,
/// or one is found but fails to parse. Both are real, free, no-API-key
/// services — the plugin works out of the box with zero configuration,
/// same promise `username-search`'s default site list makes.
pub fn default_providers() -> Vec<Provider> {
    vec![
        Provider {
            slug: "gravatar".to_string(),
            display_name: "Gravatar".to_string(),
            base_url: "https://www.gravatar.com".to_string(),
            path_template: "/avatar/{hash}?d=404".to_string(),
        },
        Provider {
            slug: "libravatar".to_string(),
            display_name: "Libravatar".to_string(),
            base_url: "https://seccdn.libravatar.org".to_string(),
            path_template: "/avatar/{hash}?d=404".to_string(),
        },
    ]
}

#[derive(Debug, Deserialize)]
struct ProvidersFile {
    #[serde(default)]
    providers: Vec<RawProvider>,
}

#[derive(Debug, Deserialize)]
struct RawProvider {
    slug: String,
    display_name: String,
    base_url: String,
    path_template: String,
}

impl From<RawProvider> for Provider {
    fn from(raw: RawProvider) -> Self {
        Provider {
            slug: raw.slug,
            display_name: raw.display_name,
            base_url: raw.base_url,
            path_template: raw.path_template,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProvidersConfigError {
    #[error("io error reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("invalid TOML in {0}: {1}")]
    Toml(PathBuf, toml::de::Error),
}

/// Parses a `providers.toml`. An empty `[[providers]]` list (present but
/// empty) is valid — it means "check nothing," the user's explicit choice
/// — only a read/parse failure is an error.
pub fn load_providers_from_path(path: &Path) -> Result<Vec<Provider>, ProvidersConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProvidersConfigError::Io(path.to_path_buf(), e))?;
    let file: ProvidersFile =
        toml::from_str(&text).map_err(|e| ProvidersConfigError::Toml(path.to_path_buf(), e))?;
    Ok(file.providers.into_iter().map(Provider::from).collect())
}

fn load_providers_or_warn(path: &Path) -> Vec<Provider> {
    match load_providers_from_path(path) {
        Ok(providers) => providers,
        Err(e) => {
            eprintln!(
                "warning: {} is invalid ({e}); falling back to the built-in provider list",
                path.display()
            );
            default_providers()
        }
    }
}

/// Resolves the provider list to check, in priority order:
/// 1. `EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE` (explicit override — tests use
///    this; so can a user who wants their config file somewhere other than
///    next to `plugin.toml`).
/// 2. `providers.toml` next to `plugin.toml`, found via
///    `EUMEAUS_PLUGIN_MANIFEST_DIR` (set by `eumeaus-plugin-host`), if
///    that file exists.
/// 3. [`default_providers`].
pub fn load_providers() -> Vec<Provider> {
    if let Ok(path) = std::env::var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE") {
        return load_providers_or_warn(Path::new(&path));
    }
    if let Ok(dir) = std::env::var("EUMEAUS_PLUGIN_MANIFEST_DIR") {
        let candidate = Path::new(&dir).join("providers.toml");
        if candidate.exists() {
            return load_providers_or_warn(&candidate);
        }
    }
    default_providers()
}

/// Gravatar/Libravatar's own documented hashing contract: MD5 of the
/// email, trimmed and lowercased first (so `" Alice@Example.com"` and
/// `"alice@example.com"` hash identically, matching how both services
/// actually match registrations).
pub fn email_hash(email: &str) -> String {
    use md5::{Digest, Md5};
    let normalized = email.trim().to_lowercase();
    format!("{:x}", Md5::digest(normalized.as_bytes()))
}

fn provider_url(provider: &Provider, hash: &str, base_override: Option<&str>) -> String {
    let path = render_path(&provider.path_template, hash);
    match base_override {
        Some(base) => format!("{}/{}{path}", base.trim_end_matches('/'), provider.slug),
        None => format!("{}{path}", provider.base_url),
    }
}

fn found(
    provider: &Provider,
    email: &str,
    hash: &str,
    source_url: &str,
) -> (Vec<EntityFinding>, Vec<RelationshipFinding>) {
    let account_key = format!("{}:{hash}", provider.slug);
    let entity = EntityFinding {
        entity_type: "OnlineAccount".to_string(),
        canonical_key: account_key.clone(),
        display_label: format!("{email} on {}", provider.display_name),
        attributes: HashMap::from([
            ("provider".to_string(), provider.display_name.clone()),
            ("avatar_url".to_string(), source_url.to_string()),
        ]),
    };
    let relationship = RelationshipFinding {
        from_canonical_key: email.to_string(),
        to_canonical_key: account_key,
        relationship_type: "HasAccount".to_string(),
    };
    (vec![entity], vec![relationship])
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as i64
}

fn provenance(source_url: String, raw_response_sha256: String) -> Provenance {
    Provenance {
        source_url,
        retrieval_method: "HTTP GET".to_string(),
        raw_response_sha256,
        collected_at_unix_ms: now_unix_ms(),
        plugin_name: "email-lookup".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Checks one provider for `email` (already hashed by the caller, so a
/// multi-provider `check()` only hashes the email once). Never panics or
/// propagates a transport error out — a request failure becomes
/// `ConfidenceStatus::Error` on this one result, per SPEC.md §5 ("one bad
/// plugin/site never aborts a scan"); the caller just moves on to the
/// next provider.
pub async fn check_provider(
    client: &reqwest::Client,
    provider: &Provider,
    email: &str,
    hash: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let url = provider_url(provider, hash, base_override);

    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return CheckResult {
                status: ConfidenceStatus::Error as i32,
                entities: vec![],
                relationships: vec![],
                provenance: Some(provenance(url, String::new())),
                error_message: e.to_string(),
            }
        }
    };

    let http_status = response.status();
    let body = response.bytes().await.unwrap_or_default();
    let raw_response_sha256 = sha256_hex(&body);

    if http_status.as_u16() == 429 {
        return CheckResult {
            status: ConfidenceStatus::Uncertain as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, raw_response_sha256)),
            error_message: String::new(),
        };
    }

    let (status, entities, relationships, error_message) = if http_status.is_success() {
        let (e, r) = found(provider, email, hash, &url);
        (ConfidenceStatus::Found, e, r, String::new())
    } else if http_status.as_u16() == 404 {
        (ConfidenceStatus::NotFound, vec![], vec![], String::new())
    } else {
        (
            ConfidenceStatus::Error,
            vec![],
            vec![],
            format!("unexpected status {http_status}"),
        )
    };

    CheckResult {
        status: status as i32,
        entities,
        relationships,
        provenance: Some(provenance(url, raw_response_sha256)),
        error_message,
    }
}

pub struct EmailLookup {
    client: reqwest::Client,
    base_override: Option<String>,
    providers: Vec<Provider>,
}

impl EmailLookup {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_EMAIL_LOOKUP_BASE_URL").ok(),
            providers: load_providers(),
        }
    }
}

impl Default for EmailLookup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for EmailLookup {
    fn describe(&self) -> (String, String) {
        (
            "email-lookup".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        let hash = email_hash(&request.input_value);
        // Sequential, not concurrent: the provider list is small, and
        // keeping this simple matters more than shaving a few hundred ms
        // off a PoC — same call `username-search` makes.
        let mut results = Vec::with_capacity(self.providers.len());
        for provider in &self.providers {
            results.push(
                check_provider(
                    &self.client,
                    provider,
                    &request.input_value,
                    &hash,
                    self.base_override.as_deref(),
                )
                .await,
            );
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `load_providers()`'s env-var-driven tests below mutate
    /// process-global state (`std::env::set_var`/`remove_var`), which
    /// races across `cargo test`'s default parallel test threads if
    /// unguarded — this serializes just those tests against each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_toml(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn email_hash_is_case_and_whitespace_insensitive() {
        assert_eq!(
            email_hash("Alice@Example.com"),
            email_hash(" alice@example.com ")
        );
    }

    #[test]
    fn email_hash_matches_the_known_gravatar_test_vector() {
        // Gravatar's own documentation uses this exact address/hash pair
        // as its worked example.
        assert_eq!(
            email_hash("MyEmailAddress@example.com "),
            "0bc83cb571cd1c50ba6f3e8a78ef1346"
        );
    }

    #[test]
    fn load_providers_from_path_parses_a_provider_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "providers.toml",
            r#"
[[providers]]
slug = "example"
display_name = "Example"
base_url = "https://avatars.example.com"
path_template = "/avatar/{hash}?d=404"
"#,
        );

        let providers = load_providers_from_path(&path).unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].slug, "example");
    }

    #[test]
    fn load_providers_from_path_allows_an_explicitly_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "providers.toml", "providers = []\n");

        let providers = load_providers_from_path(&path).unwrap();
        assert!(providers.is_empty());
    }

    #[test]
    fn load_providers_from_path_errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_providers_from_path(&dir.path().join("nope.toml")).unwrap_err();
        assert!(matches!(err, ProvidersConfigError::Io(_, _)));
    }

    #[test]
    fn load_providers_prefers_explicit_override_over_manifest_dir() {
        let _guard = ENV_LOCK.lock().unwrap();

        let override_dir = tempfile::tempdir().unwrap();
        let override_path = write_toml(
            override_dir.path(),
            "override.toml",
            r#"
[[providers]]
slug = "only-provider"
display_name = "Only Provider"
base_url = "https://only.example.com"
path_template = "/avatar/{hash}?d=404"
"#,
        );
        let manifest_dir = tempfile::tempdir().unwrap();
        write_toml(manifest_dir.path(), "providers.toml", "providers = []\n");

        // SAFETY: serialized by ENV_LOCK against every other test in this
        // module that touches process env vars.
        unsafe {
            std::env::set_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE", &override_path);
            std::env::set_var("EUMEAUS_PLUGIN_MANIFEST_DIR", manifest_dir.path());
        }

        let providers = load_providers();

        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE");
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(
            providers.len(),
            1,
            "the override file, not the manifest dir's empty list, should win"
        );
        assert_eq!(providers[0].slug, "only-provider");
    }

    #[test]
    fn load_providers_finds_providers_toml_next_to_the_manifest() {
        let _guard = ENV_LOCK.lock().unwrap();

        let manifest_dir = tempfile::tempdir().unwrap();
        write_toml(
            manifest_dir.path(),
            "providers.toml",
            r#"
[[providers]]
slug = "custom"
display_name = "Custom"
base_url = "https://custom.example.com"
path_template = "/avatar/{hash}?d=404"
"#,
        );

        // SAFETY: see load_providers_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE");
            std::env::set_var("EUMEAUS_PLUGIN_MANIFEST_DIR", manifest_dir.path());
        }

        let providers = load_providers();

        unsafe {
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].slug, "custom");
    }

    #[test]
    fn load_providers_falls_back_to_defaults_when_nothing_is_configured() {
        let _guard = ENV_LOCK.lock().unwrap();

        // SAFETY: see load_providers_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE");
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(load_providers(), default_providers());
    }

    #[test]
    fn load_providers_falls_back_to_defaults_on_a_malformed_override_file() {
        let _guard = ENV_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "providers.toml", "this is not valid toml [[[");

        // SAFETY: see load_providers_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
            std::env::set_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE", &path);
        }

        let providers = load_providers();

        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_LOOKUP_PROVIDERS_FILE");
        }

        assert_eq!(providers, default_providers());
    }
}
