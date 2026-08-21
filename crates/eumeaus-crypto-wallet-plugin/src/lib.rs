//! `eumeaus-crypto-wallet-plugin` — the fifth real plugin: looks up a
//! Bitcoin address's on-chain balance and transaction count via
//! Blockstream's Esplora API (`blockstream.info/api`) — free, keyless,
//! well-established (verified live against a known address:
//! `1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa`, real chain stats returned).
//! Bitcoin only for now — a second chain (e.g. Ethereum) would need a
//! different explorer with a different response shape and, unlike this
//! one, likely a required API key (Etherscan's free tier still needs
//! registration), which is a real enough difference to be its own
//! plugin rather than a branch in this one.
//!
//! Like `eumeaus-domain-lookup-plugin`, this enriches the *target entity
//! itself*: an `EntityFinding` with the same canonical key as the
//! scanned address auto-merges into that same entity (SPEC.md §4.4's
//! exact-key match) instead of creating a duplicate — there's no
//! separate related entity to create here, a wallet's balance is a fact
//! about the wallet, not evidence of some other thing.
//!
//! `chain_stats.tx_count == 0` (no confirmed on-chain activity ever) is
//! treated as `NotFound`, not a zero-balance `Found` — a syntactically
//! valid but never-used address is indistinguishable from a random
//! unfunded one and isn't a meaningful finding; `Found` means "this
//! wallet has verifiable on-chain history," matching the same standard
//! `eumeaus-username-search-plugin` uses for "this account exists."
//! Balance only counts confirmed chain state (`chain_stats`), not
//! `mempool_stats` — unconfirmed activity is transient and a worse fit
//! for a provenance-tracked fact than confirmed history.
//!
//! Every check is a real HTTP GET via `reqwest` — including under test,
//! where `EUMEAUS_CRYPTO_WALLET_BASE_URL` redirects requests to a local
//! mock server instead of the real domain, without changing any of the
//! request/response handling code. See `tests/`.

use std::collections::HashMap;
use std::time::Duration;

use eumeaus_plugin_protocol::{
    CheckRequest, CheckResult, ConfidenceStatus, EntityFinding, Provenance,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://blockstream.info/api";
const SATS_PER_BTC: i64 = 100_000_000;

#[derive(Debug, Deserialize)]
struct AddressResponse {
    chain_stats: ChainStats,
}

#[derive(Debug, Deserialize)]
struct ChainStats {
    funded_txo_count: i64,
    funded_txo_sum: i64,
    spent_txo_count: i64,
    spent_txo_sum: i64,
    tx_count: i64,
}

/// Formats a satoshi amount as a BTC decimal string using plain integer
/// arithmetic — floating point would risk misrepresenting a balance,
/// exactly the kind of silent-precision-loss bug this project's whole
/// provenance model exists to avoid.
fn format_btc(sats: i64) -> String {
    let whole = sats / SATS_PER_BTC;
    let frac = (sats % SATS_PER_BTC).abs();
    format!("{whole}.{frac:08}")
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
        plugin_name: "crypto-wallet".to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Turns a raw Esplora response body into a `CheckResult`. Separated
/// from the actual HTTP call so it's directly unit-testable without a
/// mock server (the mock-server tests in `tests/check.rs` cover the
/// HTTP layer itself: status codes, transport failures).
fn result_from_body(address: &str, source_url: String, body: &[u8]) -> CheckResult {
    let raw_response_sha256 = sha256_hex(body);

    let parsed: AddressResponse = match serde_json::from_slice(body) {
        Ok(v) => v,
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

    if parsed.chain_stats.tx_count == 0 {
        return CheckResult {
            status: ConfidenceStatus::NotFound as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(source_url, raw_response_sha256)),
            error_message: String::new(),
        };
    }

    let balance_sats = parsed.chain_stats.funded_txo_sum - parsed.chain_stats.spent_txo_sum;
    let attributes = HashMap::from([
        ("balance_btc".to_string(), format_btc(balance_sats)),
        (
            "tx_count".to_string(),
            parsed.chain_stats.tx_count.to_string(),
        ),
        (
            "funded_txo_count".to_string(),
            parsed.chain_stats.funded_txo_count.to_string(),
        ),
        (
            "spent_txo_count".to_string(),
            parsed.chain_stats.spent_txo_count.to_string(),
        ),
    ]);

    CheckResult {
        status: ConfidenceStatus::Found as i32,
        entities: vec![EntityFinding {
            entity_type: "CryptoWallet".to_string(),
            canonical_key: address.to_string(),
            display_label: address.to_string(),
            attributes,
        }],
        relationships: vec![],
        provenance: Some(provenance(source_url, raw_response_sha256)),
        error_message: String::new(),
    }
}

/// Checks one Bitcoin address. Never panics or propagates a transport
/// error out — a request failure becomes `ConfidenceStatus::Error`, per
/// SPEC.md §5 ("one bad plugin/site never aborts a scan").
pub async fn check_wallet(
    client: &reqwest::Client,
    address: &str,
    base_override: Option<&str>,
) -> CheckResult {
    let base = base_override.unwrap_or(DEFAULT_BASE_URL);
    let url = format!("{}/address/{}", base.trim_end_matches('/'), address.trim());

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
    if http_status.as_u16() == 429 {
        let body = response.bytes().await.unwrap_or_default();
        return CheckResult {
            status: ConfidenceStatus::Uncertain as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, sha256_hex(&body))),
            error_message: String::new(),
        };
    }
    if !http_status.is_success() {
        // Covers both a genuinely unreachable/erroring server and
        // Esplora's own `400 base58 error` for a malformed address —
        // either way this plugin failed to get a real answer, which is
        // exactly what Error means (SPEC.md §5), not a clean negative.
        let body = response.bytes().await.unwrap_or_default();
        return CheckResult {
            status: ConfidenceStatus::Error as i32,
            entities: vec![],
            relationships: vec![],
            provenance: Some(provenance(url, String::new())),
            error_message: format!(
                "unexpected status {http_status}: {}",
                String::from_utf8_lossy(&body)
            ),
        };
    }

    let body = response.bytes().await.unwrap_or_default();
    result_from_body(address, url, &body)
}

pub struct CryptoWallet {
    client: reqwest::Client,
    base_override: Option<String>,
}

impl CryptoWallet {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("building the reqwest client cannot fail with this config"),
            base_override: std::env::var("EUMEAUS_CRYPTO_WALLET_BASE_URL").ok(),
        }
    }
}

impl Default for CryptoWallet {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl eumeaus_plugin_sdk::PluginRuntime for CryptoWallet {
    fn describe(&self) -> (String, String) {
        (
            "crypto-wallet".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        )
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        vec![
            check_wallet(
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

    #[test]
    fn format_btc_handles_whole_and_fractional_amounts() {
        assert_eq!(format_btc(5_742_981_205), "57.42981205");
        assert_eq!(format_btc(0), "0.00000000");
        assert_eq!(format_btc(1), "0.00000001");
        assert_eq!(format_btc(100_000_000), "1.00000000");
    }

    fn success_body() -> &'static str {
        r#"{"chain_stats":{"funded_txo_count":78360,"funded_txo_sum":5742981205,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":65488},"mempool_stats":{"funded_txo_count":0,"funded_txo_sum":0,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":0}}"#
    }

    #[test]
    fn a_used_address_self_enriches_with_balance_and_tx_count() {
        let result = result_from_body(
            "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
            "http://example.invalid".to_string(),
            success_body().as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0];
        assert_eq!(entity.entity_type, "CryptoWallet");
        assert_eq!(entity.canonical_key, "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa");
        assert_eq!(
            entity.attributes.get("balance_btc"),
            Some(&"57.42981205".to_string())
        );
        assert_eq!(
            entity.attributes.get("tx_count"),
            Some(&"65488".to_string())
        );
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn a_never_used_address_is_not_found() {
        let body = r#"{"chain_stats":{"funded_txo_count":0,"funded_txo_sum":0,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":0},"mempool_stats":{"funded_txo_count":0,"funded_txo_sum":0,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":0}}"#;
        let result = result_from_body(
            "1BoatSLRHtKNngkdXEeobR76b53LETtpyT",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
        assert!(result.entities.is_empty());
    }

    #[test]
    fn a_spent_down_balance_is_computed_correctly() {
        // Real chain stats (verified live against 1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2):
        // funded 23643396419 sats, spent 23634701564 sats -> 8694855 sats
        // remaining = 0.08694855 BTC.
        let body = r#"{"chain_stats":{"funded_txo_count":5320,"funded_txo_sum":23643396419,"spent_txo_count":5240,"spent_txo_sum":23634701564,"tx_count":5445}}"#;
        let result = result_from_body(
            "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2",
            "http://example.invalid".to_string(),
            body.as_bytes(),
        );

        assert_eq!(result.status, ConfidenceStatus::Found as i32);
        assert_eq!(
            result.entities[0].attributes.get("balance_btc"),
            Some(&"0.08694855".to_string())
        );
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        let result = result_from_body(
            "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
            "http://example.invalid".to_string(),
            b"not json",
        );

        assert_eq!(result.status, ConfidenceStatus::Error as i32);
        assert!(!result.error_message.is_empty());
    }
}
