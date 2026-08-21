//! Exercises `check_wallet`'s HTTP layer against a real local server
//! (wiremock) standing in for Blockstream's Esplora API — real request/
//! response handling, just not the real internet. Response-body parsing/
//! entity-building logic itself is unit-tested directly in `src/lib.rs`
//! against a real captured response shape; this covers what only a real
//! HTTP round trip can: status codes, transport failures.

use eumeaus_crypto_wallet_plugin::check_wallet;
use eumeaus_plugin_protocol::ConfidenceStatus;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ADDRESS: &str = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn a_used_wallet_is_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/address/{ADDRESS}")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"chain_stats":{"funded_txo_count":1,"funded_txo_sum":100000000,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":1}}"#,
        ))
        .mount(&server)
        .await;

    let result = check_wallet(&client(), ADDRESS, Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 1);
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn an_unused_wallet_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/address/{ADDRESS}")))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"chain_stats":{"funded_txo_count":0,"funded_txo_sum":0,"spent_txo_count":0,"spent_txo_sum":0,"tx_count":0}}"#,
        ))
        .mount(&server)
        .await;

    let result = check_wallet(&client(), ADDRESS, Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn a_malformed_address_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/address/not-a-real-address"))
        .respond_with(ResponseTemplate::new(400).set_body_string("base58 error"))
        .mount(&server)
        .await;

    let result = check_wallet(&client(), "not-a-real-address", Some(&server.uri())).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(result.error_message.contains("base58 error"));
}

#[tokio::test]
async fn rate_limited_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/address/{ADDRESS}")))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_wallet(&client(), ADDRESS, Some(&server.uri())).await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Uncertain as i32,
        "SPEC.md §5: a 429 means the plugin couldn't tell, not that it failed"
    );
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_error_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the connection-failure path, not just non-2xx responses.
    let result = check_wallet(&client(), ADDRESS, Some("http://127.0.0.1:1")).await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
