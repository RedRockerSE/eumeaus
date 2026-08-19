//! `eumeaus-username-search-plugin` — the v1 proof-of-concept plugin
//! (SPEC.md §7 M5): a small Sherlock-equivalent that checks whether a
//! username exists across a configurable set of sites.
//!
//! The site list is *not* hardcoded: [`load_sites`] reads a `sites.toml`
//! (Sherlock's own `data.json` is the direct inspiration — TOML instead of
//! JSON to match the rest of this project's config) sitting next to the
//! plugin's `plugin.toml`, so a user can add or remove checks without a
//! rebuild. That directory is told to the plugin process via
//! `EUMEAUS_PLUGIN_MANIFEST_DIR` (set by `eumeaus-plugin-host`, see
//! `host.rs`'s module doc) — if that env var is unset, no `sites.toml`
//! exists there, or the file fails to parse, [`default_sites`] (today's
//! github/gitlab/flaky-forum trio) is used instead, with a warning on
//! stderr in the parse-failure case (same "degrade, don't abort" policy as
//! plugin manifest discovery, SPEC.md §5).
//!
//! **Trust boundary note:** the plugin's Ed25519 signature (SPEC.md §3.3)
//! covers only `name + version + entrypoint-binary-hash` (see
//! `eumeaus-plugin-host/src/signature.rs`) — never `sites.toml`. Editing
//! that file to add/remove a site therefore does *not* invalidate a
//! plugin's signature. That's intentional (it's meant to be freely
//! user-editable) but worth knowing: signing this plugin attests to *what
//! code runs*, not *which sites it currently checks*.
//!
//! [`Detection`] mixes the two strategies real "is this username taken"
//! checks actually need: pure HTTP status (GitHub/GitLab — 200 exists, 404
//! doesn't) and body-content inspection (a site that always answers 200
//! but says so in the page text). A 429 response is tagged `Uncertain`
//! rather than `Error` either way (SPEC.md §5: a rate limit isn't the
//! plugin failing, it's the plugin correctly declining to guess).
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_USERNAME_SEARCH_BASE_URL` redirects every site's
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
pub struct Site {
    /// Used both as the manifest-facing identifier and, in cassette mode,
    /// as the path segment that disambiguates this site on the one shared
    /// mock server (real sites each have their own domain; the override
    /// doesn't).
    pub slug: String,
    pub display_name: String,
    pub base_url: String,
    /// A path containing the literal substring `{username}`, replaced
    /// verbatim (no escaping) with the target username — e.g. `/{username}`
    /// or `/u/{username}`.
    pub path_template: String,
    pub detection: Detection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// 200 = found, 404 = not found, anything else = error.
    StatusCode,
    /// 200 always; body containing the marker means "not found" despite
    /// the 200 (some sites render a normal page either way).
    BodyMarkerMeansNotFound(String),
}

fn render_path(template: &str, username: &str) -> String {
    template.replace("{username}", username)
}

/// The built-in site list: used whenever no `sites.toml` is found, or one
/// is found but fails to parse. `flaky-forum` isn't a real site — it only
/// answers in cassette-mode tests — kept here (rather than moved into a
/// shipped `sites.toml`) so this default list is self-contained and the
/// plugin works out of the box with zero configuration.
pub fn default_sites() -> Vec<Site> {
    vec![
        Site {
            slug: "github".to_string(),
            display_name: "GitHub".to_string(),
            base_url: "https://github.com".to_string(),
            path_template: "/{username}".to_string(),
            detection: Detection::StatusCode,
        },
        Site {
            slug: "gitlab".to_string(),
            display_name: "GitLab".to_string(),
            base_url: "https://gitlab.com".to_string(),
            path_template: "/{username}".to_string(),
            detection: Detection::StatusCode,
        },
        Site {
            slug: "flaky-forum".to_string(),
            display_name: "Flaky Forum".to_string(),
            base_url: "https://flaky-forum.example.invalid".to_string(),
            path_template: "/u/{username}".to_string(),
            detection: Detection::BodyMarkerMeansNotFound("Profile not found".to_string()),
        },
    ]
}

#[derive(Debug, Deserialize)]
struct SitesFile {
    #[serde(default)]
    sites: Vec<RawSite>,
}

#[derive(Debug, Deserialize)]
struct RawSite {
    slug: String,
    display_name: String,
    base_url: String,
    path_template: String,
    /// `"status_code"` or `"body_marker"` — see [`Detection`].
    detection: String,
    #[serde(default)]
    not_found_marker: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SitesConfigError {
    #[error("io error reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("invalid TOML in {0}: {1}")]
    Toml(PathBuf, toml::de::Error),
    #[error(
        "site {0:?} has detection = \"body_marker\" but no not_found_marker (add one, or use \
         detection = \"status_code\")"
    )]
    MissingMarker(String),
    #[error(
        "site {0:?} has unknown detection {1:?} (expected \"status_code\" or \"body_marker\")"
    )]
    UnknownDetection(String, String),
}

impl RawSite {
    fn into_site(self) -> Result<Site, SitesConfigError> {
        let detection = match self.detection.as_str() {
            "status_code" => Detection::StatusCode,
            "body_marker" => Detection::BodyMarkerMeansNotFound(
                self.not_found_marker
                    .ok_or_else(|| SitesConfigError::MissingMarker(self.slug.clone()))?,
            ),
            other => {
                return Err(SitesConfigError::UnknownDetection(
                    self.slug,
                    other.to_string(),
                ))
            }
        };
        Ok(Site {
            slug: self.slug,
            display_name: self.display_name,
            base_url: self.base_url,
            path_template: self.path_template,
            detection,
        })
    }
}

/// Parses a `sites.toml`. An empty `[[sites]]` list (present but empty) is
/// valid — it means "check nothing," the user's explicit choice — only a
/// read/parse/validation failure is an error.
pub fn load_sites_from_path(path: &Path) -> Result<Vec<Site>, SitesConfigError> {
    let text =
        std::fs::read_to_string(path).map_err(|e| SitesConfigError::Io(path.to_path_buf(), e))?;
    let file: SitesFile =
        toml::from_str(&text).map_err(|e| SitesConfigError::Toml(path.to_path_buf(), e))?;
    file.sites.into_iter().map(RawSite::into_site).collect()
}

fn load_sites_or_warn(path: &Path) -> Vec<Site> {
    match load_sites_from_path(path) {
        Ok(sites) => sites,
        Err(e) => {
            eprintln!(
                "warning: {} is invalid ({e}); falling back to the built-in site list",
                path.display()
            );
            default_sites()
        }
    }
}

/// Resolves the site list to check, in priority order:
/// 1. `EUMEAUS_USERNAME_SEARCH_SITES_FILE` (explicit override — tests use
///    this; so can a user who wants their config file somewhere other than
///    next to `plugin.toml`).
/// 2. `sites.toml` next to `plugin.toml`, found via
///    `EUMEAUS_PLUGIN_MANIFEST_DIR` (set by `eumeaus-plugin-host`), if that
///    file exists.
/// 3. [`default_sites`].
pub fn load_sites() -> Vec<Site> {
    if let Ok(path) = std::env::var("EUMEAUS_USERNAME_SEARCH_SITES_FILE") {
        return load_sites_or_warn(Path::new(&path));
    }
    if let Ok(dir) = std::env::var("EUMEAUS_PLUGIN_MANIFEST_DIR") {
        let candidate = Path::new(&dir).join("sites.toml");
        if candidate.exists() {
            return load_sites_or_warn(&candidate);
        }
    }
    default_sites()
}

fn site_url(site: &Site, username: &str, base_override: Option<&str>) -> String {
    let path = render_path(&site.path_template, username);
    match base_override {
        Some(base) => format!("{}/{}{path}", base.trim_end_matches('/'), site.slug),
        None => format!("{}{path}", site.base_url),
    }
}

fn found(
    site: &Site,
    username: &str,
    source_url: &str,
) -> (Vec<EntityFinding>, Vec<RelationshipFinding>) {
    let account_key = format!("{}:{}", site.slug, username.to_lowercase());
    let entity = EntityFinding {
        entity_type: "OnlineAccount".to_string(),
        canonical_key: account_key.clone(),
        display_label: format!("{username} on {}", site.display_name),
        attributes: HashMap::from([
            ("site".to_string(), site.display_name.clone()),
            ("profile_url".to_string(), source_url.to_string()),
        ]),
    };
    let relationship = RelationshipFinding {
        from_canonical_key: username.to_string(),
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
        plugin_name: "username-search".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Checks one site for `username`. Never panics or propagates a transport
/// error out — a request failure becomes `ConfidenceStatus::Error` on this
/// one result, per SPEC.md §5 ("one bad plugin/site never aborts a scan");
/// the caller just moves on to the next site.
pub async fn check_site(
    client: &reqwest::Client,
    site: &Site,
    username: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let url = site_url(site, username, base_override);

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

    let (status, entities, relationships, error_message) = match &site.detection {
        Detection::StatusCode => {
            if http_status.is_success() {
                let (e, r) = found(site, username, &url);
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
            }
        }
        Detection::BodyMarkerMeansNotFound(marker) => {
            if !http_status.is_success() {
                (
                    ConfidenceStatus::Error,
                    vec![],
                    vec![],
                    format!("unexpected status {http_status}"),
                )
            } else if String::from_utf8_lossy(&body).contains(marker.as_str()) {
                (ConfidenceStatus::NotFound, vec![], vec![], String::new())
            } else {
                let (e, r) = found(site, username, &url);
                (ConfidenceStatus::Found, e, r, String::new())
            }
        }
    };

    CheckResult {
        status: status as i32,
        entities,
        relationships,
        provenance: Some(provenance(url, raw_response_sha256)),
        error_message,
    }
}

pub struct UsernameSearch {
    client: reqwest::Client,
    base_override: Option<String>,
    sites: Vec<Site>,
}

impl UsernameSearch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_USERNAME_SEARCH_BASE_URL").ok(),
            sites: load_sites(),
        }
    }
}

impl Default for UsernameSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for UsernameSearch {
    fn describe(&self) -> (String, String) {
        (
            "username-search".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        // Sequential, not concurrent: the site list is small, and keeping
        // this simple matters more than shaving a few hundred ms off a PoC.
        let mut results = Vec::with_capacity(self.sites.len());
        for site in &self.sites {
            results.push(
                check_site(
                    &self.client,
                    site,
                    &request.input_value,
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

    /// `load_sites()`'s env-var-driven tests below mutate process-global
    /// state (`std::env::set_var`/`remove_var`), which races across
    /// `cargo test`'s default parallel test threads if unguarded — this
    /// serializes just those tests against each other. Tests that only
    /// call `load_sites_from_path` directly (an explicit argument, no
    /// global state) don't need it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_toml(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn load_sites_from_path_parses_both_detection_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "example"
display_name = "Example"
base_url = "https://example.com"
path_template = "/{username}"
detection = "status_code"

[[sites]]
slug = "example-forum"
display_name = "Example Forum"
base_url = "https://forum.example.com"
path_template = "/u/{username}"
detection = "body_marker"
not_found_marker = "User not found"
"#,
        );

        let sites = load_sites_from_path(&path).unwrap();

        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].slug, "example");
        assert_eq!(sites[0].detection, Detection::StatusCode);
        assert_eq!(
            sites[1].detection,
            Detection::BodyMarkerMeansNotFound("User not found".to_string())
        );
    }

    #[test]
    fn load_sites_from_path_rejects_body_marker_without_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "example"
display_name = "Example"
base_url = "https://example.com"
path_template = "/{username}"
detection = "body_marker"
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(matches!(err, SitesConfigError::MissingMarker(slug) if slug == "example"));
    }

    #[test]
    fn load_sites_from_path_rejects_unknown_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "example"
display_name = "Example"
base_url = "https://example.com"
path_template = "/{username}"
detection = "regex"
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(
            matches!(err, SitesConfigError::UnknownDetection(slug, kind) if slug == "example" && kind == "regex")
        );
    }

    #[test]
    fn load_sites_from_path_allows_an_explicitly_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "sites.toml", "sites = []\n");

        let sites = load_sites_from_path(&path).unwrap();
        assert!(sites.is_empty());
    }

    #[test]
    fn load_sites_from_path_errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_sites_from_path(&dir.path().join("nope.toml")).unwrap_err();
        assert!(matches!(err, SitesConfigError::Io(_, _)));
    }

    #[test]
    fn load_sites_prefers_explicit_override_over_manifest_dir() {
        let _guard = ENV_LOCK.lock().unwrap();

        let override_dir = tempfile::tempdir().unwrap();
        let override_path = write_toml(
            override_dir.path(),
            "override.toml",
            r#"
[[sites]]
slug = "only-site"
display_name = "Only Site"
base_url = "https://only.example.com"
path_template = "/{username}"
detection = "status_code"
"#,
        );
        let manifest_dir = tempfile::tempdir().unwrap();
        write_toml(manifest_dir.path(), "sites.toml", "sites = []\n");

        // SAFETY: serialized by ENV_LOCK against every other test in this
        // module that touches process env vars.
        unsafe {
            std::env::set_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE", &override_path);
            std::env::set_var("EUMEAUS_PLUGIN_MANIFEST_DIR", manifest_dir.path());
        }

        let sites = load_sites();

        unsafe {
            std::env::remove_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE");
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(
            sites.len(),
            1,
            "the override file, not the manifest dir's empty list, should win"
        );
        assert_eq!(sites[0].slug, "only-site");
    }

    #[test]
    fn load_sites_finds_sites_toml_next_to_the_manifest() {
        let _guard = ENV_LOCK.lock().unwrap();

        let manifest_dir = tempfile::tempdir().unwrap();
        write_toml(
            manifest_dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "custom"
display_name = "Custom"
base_url = "https://custom.example.com"
path_template = "/{username}"
detection = "status_code"
"#,
        );

        // SAFETY: see load_sites_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE");
            std::env::set_var("EUMEAUS_PLUGIN_MANIFEST_DIR", manifest_dir.path());
        }

        let sites = load_sites();

        unsafe {
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].slug, "custom");
    }

    #[test]
    fn load_sites_falls_back_to_defaults_when_nothing_is_configured() {
        let _guard = ENV_LOCK.lock().unwrap();

        // SAFETY: see load_sites_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE");
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(load_sites(), default_sites());
    }

    #[test]
    fn load_sites_falls_back_to_defaults_on_a_malformed_override_file() {
        let _guard = ENV_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "sites.toml", "this is not valid toml [[[");

        // SAFETY: see load_sites_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
            std::env::set_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE", &path);
        }

        let sites = load_sites();

        unsafe {
            std::env::remove_var("EUMEAUS_USERNAME_SEARCH_SITES_FILE");
        }

        assert_eq!(sites, default_sites());
    }
}
