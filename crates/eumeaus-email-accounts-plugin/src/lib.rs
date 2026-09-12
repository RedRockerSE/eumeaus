//! `eumeaus-email-accounts-plugin` — checks whether an email address has
//! a registered account on a configurable set of sites (issue #24: a
//! ported approach from [holehe](https://github.com/megadose/holehe)).
//! Unlike `eumeaus-email-lookup-plugin` (Gravatar/Libravatar avatar
//! presence only), this checks real account existence by probing each
//! site's own signup/password-reset validation endpoint — the same
//! "is this email already taken" check the site itself runs when you
//! type an email into its signup form. No login, no notification to the
//! account holder, no API key.
//!
//! The three sites shipped as defaults (Twitter/X, Spotify, Duolingo)
//! were chosen specifically because each is a single unauthenticated GET
//! returning JSON with no CSRF-token fetch or cookie/session continuity
//! needed — verified live against the real endpoints while designing
//! this plugin, not just trusted from holehe's source. Other holehe
//! modules (GitHub, Evernote, Codepen, Imgur, ...) need a
//! GET-for-a-token-then-POST dance or HTML scraping; deliberately out of
//! scope here — [`Detection`] would need new variants to support them.
//!
//! Site list externalized the same way `eumeaus-username-search-plugin`
//! externalizes its own: see [`load_sites`] and its module doc for the
//! override/discovery/fallback order. **Trust boundary note** (same as
//! that plugin): the signature covers only name+version+entrypoint-hash,
//! never `sites.toml` — editing it doesn't invalidate the signature.
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_EMAIL_ACCOUNTS_BASE_URL` redirects every site's
//! requests to a local mock server instead of the real domain. See
//! `tests/`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Used both as the manifest-facing identifier and, in test mode, as
    /// the path segment that disambiguates this site on the one shared
    /// mock server (real sites each have their own domain; the override
    /// doesn't).
    pub slug: String,
    pub display_name: String,
    pub base_url: String,
    /// Fixed — no `{email}` templating needed. The email itself always
    /// goes in as a query parameter (`email_param`), built via
    /// `reqwest`'s own `.query(&[...])`, which percent-encodes `@`/`+`
    /// correctly for free — unlike `username-search`'s path-templating
    /// approach, which would need to hand-encode those.
    pub path: String,
    pub email_param: String,
    pub detection: Detection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// Top-level JSON boolean field; `true` = taken. (Twitter: `"taken"`)
    JsonBooleanField { field: String },
    /// Top-level JSON integer field with two *known* codes — taken iff it
    /// equals `taken_value`, not-taken iff it equals `not_taken_value`.
    /// A value that's neither is [`Outcome::Uncertain`], not a silent
    /// not-taken: this specific case was found live while building this
    /// plugin — Spotify's real endpoint sometimes answers a validly-typed
    /// `"status"` outside its documented `1`/`20` pair (observed: `100`,
    /// likely a bot-suspicion/verification-required signal, not "email is
    /// free"). holehe's own reference implementation treats exactly this
    /// the same way (only `1`/`20` are ever treated as a definite
    /// answer; anything else is `rateLimit: True, exists: None`) — this
    /// mirrors that rather than the simpler-but-wrong "only encode the
    /// taken value" version this plugin shipped with before that was
    /// caught by real end-to-end testing.
    JsonIntegerField {
        field: String,
        taken_value: i64,
        not_taken_value: i64,
    },
    /// Top-level JSON array field; non-empty = taken. (Duolingo: `"users"`)
    JsonArrayNonEmpty { field: String },
}

/// The three ways a validly-parsed JSON response can answer "is this
/// email taken" — distinct from the field being missing/wrong-typed
/// entirely, which [`Detection::evaluate`] reports as `None` (a real
/// site-shape-changed error, not any of these three).
enum Outcome {
    Taken,
    NotTaken,
    /// A recognized field with a value this rule doesn't map to either
    /// signal — maps to `ConfidenceStatus::Uncertain`, same posture as an
    /// HTTP 429 (SPEC.md §5: the plugin honestly couldn't tell, that's
    /// not the same claim as "not taken").
    Uncertain,
}

impl Detection {
    fn field_name(&self) -> &str {
        match self {
            Detection::JsonBooleanField { field } => field,
            Detection::JsonIntegerField { field, .. } => field,
            Detection::JsonArrayNonEmpty { field } => field,
        }
    }

    /// `None` when the field is missing or the wrong JSON type — treated
    /// by the caller as a site-shape-changed error (an absent field
    /// usually means the response shape moved out from under this
    /// detection rule, not that the email is free). `Some(Outcome)`
    /// otherwise, including `Outcome::Uncertain` for a validly-typed
    /// value this rule doesn't recognize as either signal.
    fn evaluate(&self, json: &serde_json::Value) -> Option<Outcome> {
        match self {
            Detection::JsonBooleanField { field } => {
                let taken = json.get(field)?.as_bool()?;
                Some(if taken {
                    Outcome::Taken
                } else {
                    Outcome::NotTaken
                })
            }
            Detection::JsonIntegerField {
                field,
                taken_value,
                not_taken_value,
            } => {
                let n = json.get(field)?.as_i64()?;
                Some(if n == *taken_value {
                    Outcome::Taken
                } else if n == *not_taken_value {
                    Outcome::NotTaken
                } else {
                    Outcome::Uncertain
                })
            }
            Detection::JsonArrayNonEmpty { field } => {
                let arr = json.get(field)?.as_array()?;
                Some(if arr.is_empty() {
                    Outcome::NotTaken
                } else {
                    Outcome::Taken
                })
            }
        }
    }
}

/// The built-in site list: used whenever no `sites.toml` is found, or one
/// is found but fails to parse. All three verified live against the real
/// endpoints (not just holehe's source) while designing this plugin.
pub fn default_sites() -> Vec<Site> {
    vec![
        Site {
            slug: "twitter".to_string(),
            display_name: "Twitter / X".to_string(),
            base_url: "https://api.twitter.com".to_string(),
            path: "/i/users/email_available.json".to_string(),
            email_param: "email".to_string(),
            detection: Detection::JsonBooleanField {
                field: "taken".to_string(),
            },
        },
        Site {
            slug: "spotify".to_string(),
            display_name: "Spotify".to_string(),
            base_url: "https://spclient.wg.spotify.com".to_string(),
            path: "/signup/public/v1/account".to_string(),
            email_param: "email".to_string(),
            detection: Detection::JsonIntegerField {
                field: "status".to_string(),
                taken_value: 20,
                not_taken_value: 1,
            },
        },
        Site {
            slug: "duolingo".to_string(),
            display_name: "Duolingo".to_string(),
            base_url: "https://www.duolingo.com".to_string(),
            path: "/2017-06-30/users".to_string(),
            email_param: "email".to_string(),
            detection: Detection::JsonArrayNonEmpty {
                field: "users".to_string(),
            },
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
    path: String,
    email_param: String,
    /// `"json_boolean_field"`, `"json_integer_field"`, or
    /// `"json_array_non_empty"` — see [`Detection`].
    detection: String,
    #[serde(default)]
    json_field: Option<String>,
    #[serde(default)]
    taken_value: Option<i64>,
    #[serde(default)]
    not_taken_value: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SitesConfigError {
    #[error("io error reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("invalid TOML in {0}: {1}")]
    Toml(PathBuf, toml::de::Error),
    #[error("site {0:?} is missing json_field")]
    MissingField(String),
    #[error("site {0:?} has detection = \"json_integer_field\" but no taken_value")]
    MissingTakenValue(String),
    #[error("site {0:?} has detection = \"json_integer_field\" but no not_taken_value")]
    MissingNotTakenValue(String),
    #[error(
        "site {0:?} has unknown detection {1:?} (expected \"json_boolean_field\", \
         \"json_integer_field\", or \"json_array_non_empty\")"
    )]
    UnknownDetection(String, String),
}

impl RawSite {
    fn into_site(self) -> Result<Site, SitesConfigError> {
        let field = || {
            self.json_field
                .clone()
                .ok_or_else(|| SitesConfigError::MissingField(self.slug.clone()))
        };
        let detection = match self.detection.as_str() {
            "json_boolean_field" => Detection::JsonBooleanField { field: field()? },
            "json_integer_field" => Detection::JsonIntegerField {
                field: field()?,
                taken_value: self
                    .taken_value
                    .ok_or_else(|| SitesConfigError::MissingTakenValue(self.slug.clone()))?,
                not_taken_value: self
                    .not_taken_value
                    .ok_or_else(|| SitesConfigError::MissingNotTakenValue(self.slug.clone()))?,
            },
            "json_array_non_empty" => Detection::JsonArrayNonEmpty { field: field()? },
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
            path: self.path,
            email_param: self.email_param,
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
/// 1. `EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE` (explicit override — tests use
///    this; so can a user who wants their config file somewhere other
///    than next to `plugin.toml`).
/// 2. `sites.toml` next to `plugin.toml`, found via
///    `EUMEAUS_PLUGIN_MANIFEST_DIR` (set by `eumeaus-plugin-host`), if
///    that file exists.
/// 3. [`default_sites`].
pub fn load_sites() -> Vec<Site> {
    if let Ok(path) = std::env::var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE") {
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

fn site_url(site: &Site, base_override: Option<&str>) -> String {
    match base_override {
        Some(base) => format!("{}/{}{}", base.trim_end_matches('/'), site.slug, site.path),
        None => format!("{}{}", site.base_url, site.path),
    }
}

fn found(
    site: &Site,
    email: &str,
    source_url: &str,
) -> (Vec<EntityFinding>, Vec<RelationshipFinding>) {
    let account_key = format!("{}:{}", site.slug, email.trim().to_lowercase());
    let entity = EntityFinding {
        entity_type: "OnlineAccount".to_string(),
        canonical_key: account_key.clone(),
        display_label: format!("{email} on {}", site.display_name),
        attributes: HashMap::from([
            ("site".to_string(), site.display_name.clone()),
            (
                "detection_method".to_string(),
                "signup-validation-probe".to_string(),
            ),
            // Not a browsable profile page (unlike username-search's own
            // "profile_url") — this is the API endpoint that confirmed the
            // finding, kept for transparency about exactly what was queried.
            ("checked_url".to_string(), source_url.to_string()),
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
        plugin_name: "email-accounts".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Checks one site for `email`. Never panics or propagates a transport
/// error out — a request failure, a non-2xx status, or a response whose
/// JSON shape doesn't match what this site's [`Detection`] expects all
/// become `ConfidenceStatus::Error` on this one result, per SPEC.md §5
/// ("one bad plugin/site never aborts a scan"); the caller just moves on
/// to the next site.
pub async fn check_site(
    client: &reqwest::Client,
    site: &Site,
    email: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let url = site_url(site, base_override);

    let response = match client
        .get(&url)
        .query(&[(site.email_param.as_str(), email)])
        .send()
        .await
    {
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

    if !http_status.is_success() {
        return CheckResult {
            status: ConfidenceStatus::Error as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, raw_response_sha256)),
            error_message: format!("unexpected status {http_status}"),
        };
    }

    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return CheckResult {
                status: ConfidenceStatus::Error as i32,
                entities: vec![],
                relationships: vec![],
                provenance: Some(provenance(url, raw_response_sha256)),
                error_message: format!("invalid JSON response: {e}"),
            }
        }
    };

    let (status, entities, relationships, error_message) = match site.detection.evaluate(&json) {
        Some(Outcome::Taken) => {
            let (e, r) = found(site, email, &url);
            (ConfidenceStatus::Found, e, r, String::new())
        }
        Some(Outcome::NotTaken) => (ConfidenceStatus::NotFound, vec![], vec![], String::new()),
        Some(Outcome::Uncertain) => (ConfidenceStatus::Uncertain, vec![], vec![], String::new()),
        None => (
            ConfidenceStatus::Error,
            vec![],
            vec![],
            format!(
                "expected field {:?} not present (or wrong type) in {}'s response — the site's \
                 API shape may have changed",
                site.detection.field_name(),
                site.display_name
            ),
        ),
    };

    CheckResult {
        status: status as i32,
        entities,
        relationships,
        provenance: Some(provenance(url, raw_response_sha256)),
        error_message,
    }
}

// How many site checks `check()` runs at once — same reasoning as
// eumeaus-username-search-plugin's own constant: a custom sites.toml
// could list many sites, so bounded-concurrent rather than sequential or
// fully parallel.
const MAX_CONCURRENT_CHECKS: usize = 20;

pub struct EmailAccounts {
    client: reqwest::Client,
    base_override: Option<String>,
    sites: Vec<Site>,
}

impl EmailAccounts {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_EMAIL_ACCOUNTS_BASE_URL").ok(),
            sites: load_sites(),
        }
    }
}

impl Default for EmailAccounts {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for EmailAccounts {
    fn describe(&self) -> (String, String) {
        (
            "email-accounts".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CHECKS));
        let mut set = JoinSet::new();
        #[allow(clippy::unnecessary_to_owned)]
        for site in self.sites.iter().cloned() {
            let semaphore = Arc::clone(&semaphore);
            let client = self.client.clone();
            let email = request.input_value.clone();
            let base_override = self.base_override.clone();
            set.spawn(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .expect("semaphore is never closed");
                check_site(&client, &site, &email, base_override.as_deref()).await
            });
        }

        let mut results = Vec::with_capacity(self.sites.len());
        while let Some(joined) = set.join_next().await {
            results.push(joined.expect("check_site task panicked"));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_plugin_sdk::PluginRuntime;
    use std::sync::Mutex;

    /// `load_sites()`'s env-var-driven tests below mutate process-global
    /// state, which races across `cargo test`'s default parallel test
    /// threads if unguarded.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_toml(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn load_sites_from_path_parses_all_three_detection_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "a"
display_name = "A"
base_url = "https://a.example.com"
path = "/check"
email_param = "email"
detection = "json_boolean_field"
json_field = "taken"

[[sites]]
slug = "b"
display_name = "B"
base_url = "https://b.example.com"
path = "/check"
email_param = "email"
detection = "json_integer_field"
json_field = "status"
taken_value = 20
not_taken_value = 1

[[sites]]
slug = "c"
display_name = "C"
base_url = "https://c.example.com"
path = "/check"
email_param = "email"
detection = "json_array_non_empty"
json_field = "users"
"#,
        );

        let sites = load_sites_from_path(&path).unwrap();

        assert_eq!(sites.len(), 3);
        assert_eq!(
            sites[0].detection,
            Detection::JsonBooleanField {
                field: "taken".to_string()
            }
        );
        assert_eq!(
            sites[1].detection,
            Detection::JsonIntegerField {
                field: "status".to_string(),
                taken_value: 20,
                not_taken_value: 1,
            }
        );
        assert_eq!(
            sites[2].detection,
            Detection::JsonArrayNonEmpty {
                field: "users".to_string()
            }
        );
    }

    #[test]
    fn load_sites_from_path_rejects_missing_json_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "a"
display_name = "A"
base_url = "https://a.example.com"
path = "/check"
email_param = "email"
detection = "json_boolean_field"
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(matches!(err, SitesConfigError::MissingField(slug) if slug == "a"));
    }

    #[test]
    fn load_sites_from_path_rejects_integer_field_without_taken_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "a"
display_name = "A"
base_url = "https://a.example.com"
path = "/check"
email_param = "email"
detection = "json_integer_field"
json_field = "status"
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(matches!(err, SitesConfigError::MissingTakenValue(slug) if slug == "a"));
    }

    #[test]
    fn load_sites_from_path_rejects_integer_field_without_not_taken_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "a"
display_name = "A"
base_url = "https://a.example.com"
path = "/check"
email_param = "email"
detection = "json_integer_field"
json_field = "status"
taken_value = 20
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(matches!(err, SitesConfigError::MissingNotTakenValue(slug) if slug == "a"));
    }

    #[test]
    fn load_sites_from_path_rejects_unknown_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(
            dir.path(),
            "sites.toml",
            r#"
[[sites]]
slug = "a"
display_name = "A"
base_url = "https://a.example.com"
path = "/check"
email_param = "email"
detection = "regex"
"#,
        );

        let err = load_sites_from_path(&path).unwrap_err();
        assert!(
            matches!(err, SitesConfigError::UnknownDetection(slug, kind) if slug == "a" && kind == "regex")
        );
    }

    #[test]
    fn load_sites_from_path_allows_an_explicitly_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(dir.path(), "sites.toml", "sites = []\n");

        assert!(load_sites_from_path(&path).unwrap().is_empty());
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
path = "/check"
email_param = "email"
detection = "json_boolean_field"
json_field = "taken"
"#,
        );
        let manifest_dir = tempfile::tempdir().unwrap();
        write_toml(manifest_dir.path(), "sites.toml", "sites = []\n");

        // SAFETY: serialized by ENV_LOCK against every other test in this
        // module that touches process env vars.
        unsafe {
            std::env::set_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE", &override_path);
            std::env::set_var("EUMEAUS_PLUGIN_MANIFEST_DIR", manifest_dir.path());
        }

        let sites = load_sites();

        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE");
            std::env::remove_var("EUMEAUS_PLUGIN_MANIFEST_DIR");
        }

        assert_eq!(sites.len(), 1);
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
path = "/check"
email_param = "email"
detection = "json_boolean_field"
json_field = "taken"
"#,
        );

        // SAFETY: see load_sites_prefers_explicit_override_over_manifest_dir.
        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE");
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
            std::env::remove_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE");
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
            std::env::set_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE", &path);
        }

        let sites = load_sites();

        unsafe {
            std::env::remove_var("EUMEAUS_EMAIL_ACCOUNTS_SITES_FILE");
        }

        assert_eq!(sites, default_sites());
    }

    #[tokio::test]
    async fn check_handles_more_sites_than_the_concurrency_cap() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"taken": true})),
            )
            .mount(&server)
            .await;

        let sites: Vec<Site> = (0..MAX_CONCURRENT_CHECKS * 2 + 1)
            .map(|i| Site {
                slug: format!("site-{i}"),
                display_name: format!("Site {i}"),
                base_url: "https://example.invalid".to_string(),
                path: "/check".to_string(),
                email_param: "email".to_string(),
                detection: Detection::JsonBooleanField {
                    field: "taken".to_string(),
                },
            })
            .collect();
        let expected = sites.len();

        let search = EmailAccounts {
            client: reqwest::Client::new(),
            base_override: Some(server.uri()),
            sites,
        };

        let results = search
            .check(&CheckRequest {
                input_value: "carol@example.com".to_string(),
                ..Default::default()
            })
            .await;

        assert_eq!(results.len(), expected);
        assert!(results
            .iter()
            .all(|r| r.status == ConfidenceStatus::Found as i32));
    }
}
