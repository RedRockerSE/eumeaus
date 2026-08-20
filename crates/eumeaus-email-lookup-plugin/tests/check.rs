//! Exercises `check_provider`'s detection logic against a real local HTTP
//! server (wiremock) standing in for each real provider — real
//! request/response handling, just not the real internet. This is the
//! "cassette" SPEC.md §6 calls for: recorded/canned responses replayed
//! deterministically.

use eumeaus_email_lookup_plugin::{check_provider, default_providers, email_hash, Provider};
use eumeaus_plugin_protocol::ConfidenceStatus;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider(slug: &str) -> Provider {
    default_providers()
        .into_iter()
        .find(|p| p.slug == slug)
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn provider_found() {
    let server = MockServer::start().await;
    let hash = email_hash("carol@example.com");
    Mock::given(method("GET"))
        .and(path(format!("/gravatar/avatar/{hash}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4]))
        .mount(&server)
        .await;

    let result = check_provider(
        &client(),
        &provider("gravatar"),
        "carol@example.com",
        &hash,
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].canonical_key, format!("gravatar:{hash}"));
    assert_eq!(result.relationships.len(), 1);
    assert_eq!(
        result.relationships[0].from_canonical_key,
        "carol@example.com"
    );
    assert_eq!(
        result.relationships[0].to_canonical_key,
        format!("gravatar:{hash}")
    );
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn provider_not_found() {
    let server = MockServer::start().await;
    let hash = email_hash("nobody@example.com");
    Mock::given(method("GET"))
        .and(path(format!("/gravatar/avatar/{hash}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let result = check_provider(
        &client(),
        &provider("gravatar"),
        "nobody@example.com",
        &hash,
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[tokio::test]
async fn email_casing_and_whitespace_do_not_change_the_looked_up_hash() {
    let server = MockServer::start().await;
    let hash = email_hash("carol@example.com");
    Mock::given(method("GET"))
        .and(path(format!("/gravatar/avatar/{hash}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4]))
        .mount(&server)
        .await;

    let messy_hash = email_hash(" Carol@Example.com ");
    assert_eq!(hash, messy_hash);

    let result = check_provider(
        &client(),
        &provider("gravatar"),
        " Carol@Example.com ",
        &messy_hash,
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
}

#[tokio::test]
async fn rate_limited_provider_is_uncertain_not_error() {
    let server = MockServer::start().await;
    let hash = email_hash("carol@example.com");
    Mock::given(method("GET"))
        .and(path(format!("/gravatar/avatar/{hash}")))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_provider(
        &client(),
        &provider("gravatar"),
        "carol@example.com",
        &hash,
        Some(&server.uri()),
    )
    .await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Uncertain as i32,
        "SPEC.md §5: a 429 means the plugin couldn't tell, not that it failed"
    );
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn unexpected_status_is_error() {
    let server = MockServer::start().await;
    let hash = email_hash("carol@example.com");
    Mock::given(method("GET"))
        .and(path(format!("/gravatar/avatar/{hash}")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let result = check_provider(
        &client(),
        &provider("gravatar"),
        "carol@example.com",
        &hash,
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn unreachable_host_is_error_not_a_panic() {
    // Port 1 is reserved and nothing will ever be listening there — this
    // exercises the connection-failure path, not just non-2xx responses.
    let hash = email_hash("carol@example.com");
    let result = check_provider(
        &client(),
        &provider("gravatar"),
        "carol@example.com",
        &hash,
        Some("http://127.0.0.1:1"),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
