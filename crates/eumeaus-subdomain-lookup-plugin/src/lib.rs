//! `eumeaus-subdomain-lookup-plugin` — enumerates a `Domain` entity's
//! subdomains via Certificate Transparency logs, using crt.sh's free,
//! no-API-key JSON search endpoint (GitHub issue #25).
//!
//! Complements `eumeaus-domain-lookup-plugin` (RDAP registration data):
//! this plugin answers "what's actually hosted under this domain?"
//! instead of "who registered it?".
//!
//! Like `eumeaus-ip-lookup-plugin`, a single HTTP call can produce many
//! findings — every certificate crt.sh has ever logged for the domain,
//! deduplicated into a set of subdomain names. Two things distinguish
//! this plugin from every other shipped one so far:
//!
//! - crt.sh is a public, Postgres-backed search over a huge and
//!   constantly growing dataset, and is genuinely flaky in practice —
//!   live testing during development saw both a bare `502 Bad Gateway`
//!   and outright connection failures on a first attempt, each
//!   succeeding on a bare retry moments later with no code change. This
//!   plugin retries a bounded number of times before giving up, and
//!   giving up is reported as `Uncertain` (crt.sh being unavailable
//!   isn't evidence the domain has no subdomains), not `Error`.
//! - A popular domain can have thousands of unique subdomains logged
//!   (`cloudflare.com` returned 3,425 in live testing) — `MAX_SUBDOMAINS`
//!   caps how many become new case entities in one scan, per SPEC.md
//!   §5's "degrade, don't abort" posture for oversized results.
//!
//! A real, live end-to-end run against `example.com` (not a mocked test)
//! caught a genuine correctness bug during development: crt.sh's
//! `name_value` field doesn't only carry DNS hostnames — that one
//! response included an email-address SAN (`"user@example.com"`) and a
//! CA intermediate certificate's subject common name
//! (`"AS207960 Test Intermediate - example.com"`), neither of which is a
//! subdomain. `looks_like_hostname` filters both out; see its doc
//! comment and `drops_non_hostname_name_value_entries` in `tests`.
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_SUBDOMAIN_LOOKUP_BASE_URL` redirects requests to a
//! local mock server instead of the real domain, without changing any of
//! the request/response handling code. See `tests/`.

use std::collections::HashSet;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance, RelationshipFinding,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://crt.sh";

/// Real numbers from live testing while building this plugin:
/// `cloudflare.com` alone returned 3,425 unique hostnames. Emitting all
/// of them as new case entities from one scan isn't what an investigator
/// wants by default — this caps it, with a stderr warning when
/// truncated (same "degrade, don't abort" posture as a bad `sites.toml`
/// elsewhere), not a hard error.
const MAX_SUBDOMAINS: usize = 200;

/// A bare transport error or non-2xx status is retried this many times
/// (in addition to the first attempt) before giving up as `Uncertain` —
/// crt.sh's well-documented flakiness under normal load, confirmed live
/// during development (a `502`, and separately a connection failure,
/// each succeeded on a bare retry with no code change).
const MAX_RETRIES: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_millis(1500);

#[derive(Debug, Deserialize)]
struct CrtShRecord {
    name_value: String,
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
        plugin_name: "subdomain-lookup".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// `name_value` doesn't only carry DNS hostnames, confirmed by a live
/// response for `example.com` during development: it also included
/// `"user@example.com"` (an email-address SAN on an S/MIME-style cert)
/// and `"AS207960 Test Intermediate - example.com"` (a CA intermediate
/// certificate's *subject common name*, not a SAN entry at all — crt.sh
/// falls back to it when a cert has no SANs). Neither is a subdomain;
/// both would otherwise become bogus `Domain` entities. A real hostname
/// label only ever contains letters, digits, hyphens, and (for things
/// like `_dmarc.example.com` TXT-record conventions) underscores — no
/// spaces, `@`, or other punctuation — so anything else is dropped here
/// rather than trusted at face value.
fn looks_like_hostname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// Parses a raw crt.sh JSON response body into a sorted, deduplicated
/// list of subdomains of `domain`. Pure and separately unit-testable,
/// mirroring `eumeaus-ip-lookup-plugin`'s `result_from_body` split.
///
/// Each record's `name_value` field is a newline-joined string of every
/// SAN on that certificate (e.g. `"*.example.com\nexample.com"`), not a
/// single hostname — confirmed via live testing, not assumed from
/// theHarvester's source. Every name is trimmed, lowercased, and has one
/// leading `*.` stripped if present (a wildcard cert for
/// `*.foo.example.com` is treated as evidence `foo.example.com` exists);
/// anything still starting with `*`, equal to the scanned domain itself,
/// or not shaped like a real hostname (see `looks_like_hostname`) is
/// dropped.
fn subdomains_from_body(domain: &str, body: &[u8]) -> Result<Vec<String>, serde_json::Error> {
    let records: Vec<CrtShRecord> = serde_json::from_slice(body)?;
    let domain_lower = domain.trim().to_lowercase();

    let mut names: HashSet<String> = HashSet::new();
    for record in &records {
        for raw in record.name_value.split('\n') {
            let mut name = raw.trim().to_lowercase();
            if let Some(stripped) = name.strip_prefix("*.") {
                name = stripped.to_string();
            }
            if name.is_empty()
                || name.starts_with('*')
                || name == domain_lower
                || !looks_like_hostname(&name)
            {
                continue;
            }
            names.insert(name);
        }
    }

    let mut sorted: Vec<String> = names.into_iter().collect();
    sorted.sort();
    if sorted.len() > MAX_SUBDOMAINS {
        eprintln!(
            "subdomain-lookup: crt.sh returned {} unique subdomains for {domain}, \
             showing only the first {MAX_SUBDOMAINS}",
            sorted.len()
        );
        sorted.truncate(MAX_SUBDOMAINS);
    }
    Ok(sorted)
}

/// Turns a raw crt.sh response body into a `CheckResult`. Separated from
/// the actual HTTP call so it's directly unit-testable without a mock
/// server (the mock-server tests in `tests/check.rs` cover the HTTP
/// layer itself: status codes, transport failures, retries).
fn result_from_body(domain: &str, source_url: String, body: &[u8]) -> CheckResult {
    let raw_response_sha256 = sha256_hex(body);

    let subdomains = match subdomains_from_body(domain, body) {
        Ok(subdomains) => subdomains,
        Err(e) => {
            return CheckResult {
                status: ConfidenceStatus::Error as i32,
                entities: vec![],
                relationships: vec![],
                provenance: Some(provenance(source_url, raw_response_sha256)),
                error_message: format!("invalid response body: {e}"),
            }
        }
    };

    if subdomains.is_empty() {
        // crt.sh answered definitively — it just has nothing logged for
        // this domain. Distinct from the retry-exhausted Uncertain case.
        return CheckResult {
            status: ConfidenceStatus::NotFound as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(source_url, raw_response_sha256)),
            error_message: String::new(),
        };
    }

    let mut entities = Vec::with_capacity(subdomains.len());
    let mut relationships = Vec::with_capacity(subdomains.len());
    for subdomain in &subdomains {
        entities.push(EntityFinding {
            entity_type: "Domain".to_string(),
            canonical_key: subdomain.clone(),
            display_label: subdomain.clone(),
            attributes: std::collections::HashMap::from([
                ("source".to_string(), "crt.sh".to_string()),
                (
                    "discovered_via".to_string(),
                    "certificate transparency".to_string(),
                ),
            ]),
        });
        relationships.push(RelationshipFinding {
            from_canonical_key: domain.to_string(),
            to_canonical_key: subdomain.clone(),
            relationship_type: "HasSubdomain".to_string(),
        });
    }

    CheckResult {
        status: ConfidenceStatus::Found as i32,
        entities,
        relationships,
        provenance: Some(provenance(source_url, raw_response_sha256)),
        error_message: String::new(),
    }
}

fn uncertain_result(url: String, raw_response_sha256: String) -> CheckResult {
    CheckResult {
        status: ConfidenceStatus::Uncertain as i32,
        entities: vec![],
        relationships: vec![],
        provenance: Some(provenance(url, raw_response_sha256)),
        error_message: String::new(),
    }
}

/// Checks one domain. Never panics or propagates a transport error out —
/// a request failure, or crt.sh's own rate limiting, becomes
/// `ConfidenceStatus::Uncertain` after retries are exhausted, per
/// SPEC.md §5 ("one bad plugin/site never aborts a scan").
pub async fn check_domain(
    client: &reqwest::Client,
    domain: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let base = base_override.unwrap_or(DEFAULT_BASE_URL);
    let url = format!(
        "{}/?q={domain}&exclude=expired&output=json",
        base.trim_end_matches('/')
    );

    for attempt in 0..=MAX_RETRIES {
        let response = match client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => {
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                return uncertain_result(url, String::new());
            }
        };

        let http_status = response.status();
        if http_status.as_u16() == 429 {
            let body = response.bytes().await.unwrap_or_default();
            return uncertain_result(url, sha256_hex(&body));
        }
        if !http_status.is_success() {
            if attempt < MAX_RETRIES {
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            return uncertain_result(url, String::new());
        }

        let body = response.bytes().await.unwrap_or_default();
        return result_from_body(domain, url, &body);
    }

    unreachable!("the loop above always returns by its last iteration")
}

pub struct SubdomainLookup {
    client: reqwest::Client,
    base_override: Option<String>,
}

impl SubdomainLookup {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_SUBDOMAIN_LOOKUP_BASE_URL").ok(),
        }
    }
}

impl Default for SubdomainLookup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for SubdomainLookup {
    fn describe(&self) -> (String, String) {
        (
            "subdomain-lookup".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        vec![
            check_domain(
                &self.client,
                &request.input_value,
                self.base_override.as_deref(),
            )
            .await,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crt_sh_body(names: &[&str]) -> String {
        let records: Vec<serde_json::Value> = names
            .iter()
            .map(|n| serde_json::json!({"name_value": n}))
            .collect();
        serde_json::to_string(&records).unwrap()
    }

    #[test]
    fn splits_multi_san_name_value_and_dedupes() {
        let body = crt_sh_body(&["a.example.com\nb.example.com", "a.example.com"]);
        let subdomains = subdomains_from_body("example.com", body.as_bytes()).unwrap();
        assert_eq!(subdomains, vec!["a.example.com", "b.example.com"]);
    }

    #[test]
    fn strips_one_leading_wildcard_prefix() {
        let body = crt_sh_body(&["*.foo.example.com"]);
        let subdomains = subdomains_from_body("example.com", body.as_bytes()).unwrap();
        assert_eq!(subdomains, vec!["foo.example.com"]);
    }

    #[test]
    fn drops_names_still_starting_with_a_wildcard() {
        // A cert whose SAN is a bare "*" (unusual, but seen in the wild)
        // shouldn't produce an empty or malformed entity.
        let body = crt_sh_body(&["*", "*.*.example.com"]);
        let subdomains = subdomains_from_body("example.com", body.as_bytes()).unwrap();
        assert!(subdomains.is_empty());
    }

    /// Regression test for a real finding from live end-to-end testing:
    /// a real crt.sh response for `example.com` included
    /// `"user@example.com"` (an email-address SAN) and
    /// `"AS207960 Test Intermediate - example.com"` (a CA intermediate
    /// certificate's subject common name, not a SAN at all). Neither is
    /// a subdomain — treating every `name_value` entry as one, this
    /// plugin's first version's actual bug, would have created bogus
    /// `Domain` entities for both.
    #[test]
    fn drops_non_hostname_name_value_entries() {
        let body = crt_sh_body(&[
            "example.com\nuser@example.com",
            "AS207960 Test Intermediate - example.com",
            "www.example.com",
        ]);
        let subdomains = subdomains_from_body("example.com", body.as_bytes()).unwrap();
        assert_eq!(subdomains, vec!["www.example.com"]);
    }

    #[test]
    fn drops_the_scanned_domain_itself() {
        let body = crt_sh_body(&["example.com", "www.example.com"]);
        let subdomains = subdomains_from_body("Example.com", body.as_bytes()).unwrap();
        assert_eq!(subdomains, vec!["www.example.com"]);
    }

    #[test]
    fn caps_at_max_subdomains() {
        let names: Vec<String> = (0..(MAX_SUBDOMAINS + 50))
            .map(|i| format!("sub{i}.example.com"))
            .collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let body = crt_sh_body(&name_refs);
        let subdomains = subdomains_from_body("example.com", body.as_bytes()).unwrap();
        assert_eq!(subdomains.len(), MAX_SUBDOMAINS);
    }

    #[test]
    fn malformed_body_is_a_parse_error() {
        subdomains_from_body("example.com", b"<html>not json</html>").unwrap_err();
    }

    #[test]
    fn empty_result_set_produces_not_found() {
        let body = crt_sh_body(&[]);
        let result = result_from_body(
            "example.com",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );
        assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
        assert!(result.entities.is_empty());
    }

    #[test]
    fn a_hit_produces_domain_entities_and_has_subdomain_relationships() {
        let body = crt_sh_body(&["www.example.com", "api.example.com"]);
        let result = result_from_body(
            "example.com",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.relationships.len(), 2);

        let www = result
            .entities
            .iter()
            .find(|e| e.canonical_key == "www.example.com")
            .unwrap();
        assert_eq!(www.entity_type, "Domain");
        assert_eq!(www.attributes.get("source"), Some(&"crt.sh".to_string()));

        let rel = result
            .relationships
            .iter()
            .find(|r| r.to_canonical_key == "www.example.com")
            .unwrap();
        assert_eq!(rel.from_canonical_key, "example.com");
        assert_eq!(rel.relationship_type, "HasSubdomain");
    }

    #[test]
    fn malformed_json_on_success_is_an_error_result() {
        let result = result_from_body(
            "example.com",
            "http://example.invalid".to_string(),
            b"<html>not json</html>",
        );
        assert_eq!(result.status, ConfidenceStatus::Error as i32);
        assert!(!result.error_message.is_empty());
    }
}
