//! `eumeaus-username-search-plugin` — the v1 proof-of-concept plugin
//! (SPEC.md §7 M5): a small Sherlock-equivalent that checks whether a
//! username exists across a handful of sites.
//!
//! [`SITES`] mixes the two detection strategies real "is this username
//! taken" checks actually need: pure HTTP status (GitHub/GitLab — 200
//! exists, 404 doesn't) and body-content inspection (a site that always
//! answers 200 but says so in the page text). A 429 response is tagged
//! `Uncertain` rather than `Error` either way (SPEC.md §5: a rate limit
//! isn't the plugin failing, it's the plugin correctly declining to guess).
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_USERNAME_SEARCH_BASE_URL` redirects every site's
//! requests to a local mock server instead of the real domain, without
//! changing any of the request/response handling code. See `tests/`.

use std::collections::HashMap;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};

pub struct Site {
    /// Used both as the manifest-facing identifier and, in cassette mode,
    /// as the path segment that disambiguates this site on the one shared
    /// mock server (real sites each have their own domain; the override
    /// doesn't).
    pub slug: &'static str,
    pub display_name: &'static str,
    pub real_origin: &'static str,
    pub path: fn(&str) -> String,
    pub detection: Detection,
}

pub enum Detection {
    /// 200 = found, 404 = not found, anything else = error.
    StatusCode,
    /// 200 always; body containing `marker` means "not found" despite the
    /// 200 (some sites render a normal page either way).
    BodyMarkerMeansNotFound(&'static str),
}

pub const SITES: &[Site] = &[
    Site {
        slug: "github",
        display_name: "GitHub",
        real_origin: "https://github.com",
        path: |u| format!("/{u}"),
        detection: Detection::StatusCode,
    },
    Site {
        slug: "gitlab",
        display_name: "GitLab",
        real_origin: "https://gitlab.com",
        path: |u| format!("/{u}"),
        detection: Detection::StatusCode,
    },
    Site {
        slug: "flaky-forum",
        display_name: "Flaky Forum",
        // Not a real site — only ever reached in cassette mode, to
        // demonstrate content-based detection (SPEC.md §3.2's illustrative
        // protocol doesn't restrict a plugin to status-code checks only).
        real_origin: "https://flaky-forum.example.invalid",
        path: |u| format!("/u/{u}"),
        detection: Detection::BodyMarkerMeansNotFound("Profile not found"),
    },
];

fn site_url(site: &Site, username: &str, base_override: Option<&str>) -> String {
    match base_override {
        Some(base) => format!(
            "{}/{}{}",
            base.trim_end_matches('/'),
            site.slug,
            (site.path)(username)
        ),
        None => format!("{}{}", site.real_origin, (site.path)(username)),
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
            ("site".to_string(), site.display_name.to_string()),
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

    let (status, entities, relationships, error_message) = match site.detection {
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
            } else if String::from_utf8_lossy(&body).contains(marker) {
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
}

impl UsernameSearch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_USERNAME_SEARCH_BASE_URL").ok(),
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
        // Sequential, not concurrent: SITES is small (3), and keeping this
        // simple matters more than shaving a few hundred ms off a PoC.
        let mut results = Vec::with_capacity(SITES.len());
        for site in SITES {
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
