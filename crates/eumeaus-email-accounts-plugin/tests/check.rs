//! Exercises `check_site`'s detection logic against a real local HTTP
//! server (wiremock) standing in for each site — real request/response
//! handling, just not the real internet. Mirrors
//! `eumeaus-username-search-plugin/tests/check.rs`'s own pattern, one
//! test per `Detection` variant plus the shared error paths.

use eumeaus_email_accounts_plugin::{check_site, default_sites, Site};
use eumeaus_plugin_protocol::ConfidenceStatus;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn site(slug: &str) -> Site {
    default_sites()
        .into_iter()
        .find(|s| s.slug == slug)
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn json_boolean_field_site_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .and(query_param("email", "carol@example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"taken": true})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(
        result.entities[0].canonical_key,
        "twitter:carol@example.com"
    );
    assert_eq!(result.relationships.len(), 1);
    assert_eq!(
        result.relationships[0].from_canonical_key,
        "carol@example.com"
    );
    assert_eq!(
        result.relationships[0].to_canonical_key,
        "twitter:carol@example.com"
    );
    let prov = result.provenance.expect("provenance recorded");
    assert!(!prov.raw_response_sha256.is_empty());
}

#[tokio::test]
async fn json_boolean_field_site_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"taken": false})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "nobody@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[tokio::test]
async fn json_integer_field_site_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spotify/signup/public/v1/account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": 20})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("spotify"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(
        result.entities[0].canonical_key,
        "spotify:carol@example.com"
    );
}

#[tokio::test]
async fn json_integer_field_site_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spotify/signup/public/v1/account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": 1})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("spotify"),
        "nobody@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

/// Regression test for a real finding from live end-to-end testing:
/// Spotify's actual endpoint sometimes answers a validly-typed `"status"`
/// outside its documented `1`/`20` pair (observed: `100`, likely a bot-
/// suspicion/verification-required signal). Mapping that to a confident
/// `NotFound` — this plugin's first version's actual bug — would silently
/// misreport an email as free when the plugin genuinely couldn't tell.
#[tokio::test]
async fn json_integer_field_with_an_unrecognized_status_is_uncertain_not_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/spotify/signup/public/v1/account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": 100})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("spotify"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Uncertain as i32,
        "a recognized-but-unmapped status code is 'couldn't tell', not 'not taken'"
    );
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn json_array_non_empty_site_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/duolingo/2017-06-30/users"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"users": [{"id": 1}]})),
        )
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("duolingo"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Found as i32);
    assert_eq!(
        result.entities[0].canonical_key,
        "duolingo:carol@example.com"
    );
}

#[tokio::test]
async fn json_array_non_empty_site_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/duolingo/2017-06-30/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"users": []})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("duolingo"),
        "nobody@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::NotFound as i32);
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn malformed_json_body_is_error_not_a_panic() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}

#[tokio::test]
async fn missing_expected_field_is_error_not_a_silent_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"unrelated": 1})))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
        Some(&server.uri()),
    )
    .await;

    assert_eq!(
        result.status,
        ConfidenceStatus::Error as i32,
        "a response shape that no longer matches the detection rule is a site-changed error, \
         not a definite not-found"
    );
    assert!(result.error_message.contains("taken"));
}

#[tokio::test]
async fn rate_limited_site_is_uncertain_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
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
    Mock::given(method("GET"))
        .and(path("/twitter/i/users/email_available.json"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
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
    let result = check_site(
        &client(),
        &site("twitter"),
        "carol@example.com",
        Some("http://127.0.0.1:1"),
    )
    .await;

    assert_eq!(result.status, ConfidenceStatus::Error as i32);
    assert!(!result.error_message.is_empty());
}
