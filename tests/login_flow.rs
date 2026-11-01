//! End-to-end login flow against a mocked Kite API.

use std::sync::Arc;

use kite_mcp::auth::Auth;
use kite_mcp::config::Config;
use kite_mcp::kite::KiteClient;
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn setup() -> (MockServer, tempfile::TempDir, Arc<KiteClient>, Arc<Auth>) {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        api_key: "key".into(),
        api_secret: "secret".into(),
        callback_port: free_port(),
        api_base: server.uri(),
        home: home.path().to_path_buf(),
        ..Config::default()
    };
    let client = Arc::new(KiteClient::new(&config.api_base, &config.api_key).unwrap());
    let auth = Auth::new(config, client.clone());
    (server, home, client, auth)
}

async fn mock_session_and_profile(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/session/token"))
        .and(body_string_contains("request_token=req123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "data": {"user_id": "AB1234", "user_name": "Test User", "access_token": "acc456"}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/profile"))
        .and(header("Authorization", "token key:acc456"))
        .and(header("X-Kite-Version", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "data": {"user_id": "AB1234", "user_name": "Test User", "exchanges": ["NSE", "BSE"]}
        })))
        .mount(server)
        .await;
}

fn state_from(url: &str) -> String {
    url.split("state%3D").nth(1).unwrap().to_string()
}

#[tokio::test]
async fn callback_login_then_profile() {
    let (server, home, client, auth) = setup().await;
    mock_session_and_profile(&server).await;

    let start = auth.start_login().await.unwrap();
    assert!(start.callback_listening);
    assert!(start.login_url.contains("api_key=key"));

    let cb = format!(
        "{}?action=login&status=success&request_token=req123&state={}",
        start.redirect_url,
        state_from(&start.login_url)
    );
    let page = reqwest::get(&cb).await.unwrap().text().await.unwrap();
    assert!(page.contains("Logged in to Kite"), "{page}");

    let status = auth.status();
    assert!(status.logged_in);
    assert_eq!(status.user_id.as_deref(), Some("AB1234"));
    assert!(home.path().join("session.json").exists());

    let profile = client.profile().await.unwrap();
    assert_eq!(profile.exchanges, vec!["NSE", "BSE"]);
}

#[tokio::test]
async fn callback_rejects_wrong_state() {
    let (server, _home, _client, auth) = setup().await;
    mock_session_and_profile(&server).await;

    let start = auth.start_login().await.unwrap();
    let cb = format!(
        "{}?status=success&request_token=req123&state=forged",
        start.redirect_url
    );
    let page = reqwest::get(&cb).await.unwrap().text().await.unwrap();
    assert!(page.contains("Login rejected"));
    assert!(!auth.status().logged_in);
}

#[tokio::test]
async fn manual_login_restores_after_restart() {
    let (server, home, _client, auth) = setup().await;
    mock_session_and_profile(&server).await;
    auth.complete_login("req123").await.unwrap();

    // A fresh process with the same home picks the session back up.
    let config = Config::load_from(home.path()).unwrap();
    let config = Config {
        api_key: "key".into(),
        api_secret: "secret".into(),
        api_base: server.uri(),
        ..config
    };
    let client2 = Arc::new(KiteClient::new(&config.api_base, &config.api_key).unwrap());
    let auth2 = Auth::new(config, client2.clone());
    auth2.restore().unwrap();
    assert!(auth2.status().logged_in);
    assert_eq!(client2.profile().await.unwrap().user_id, "AB1234");
}

#[tokio::test]
async fn token_exception_is_reported() {
    let (server, _home, client, _auth) = setup().await;
    client.set_access_token(Some("stale".into()));
    Mock::given(method("GET"))
        .and(path("/user/profile"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "status": "error", "error_type": "TokenException", "message": "Token expired"
        })))
        .mount(&server)
        .await;
    let err = client.profile().await.unwrap_err();
    assert!(err.is_token_error());
}

#[tokio::test]
async fn profile_without_login_is_not_logged_in() {
    let (_server, _home, client, _auth) = setup().await;
    let err = client.profile().await.unwrap_err();
    assert!(err.is_token_error());
    assert!(err.to_string().contains("login"));
}

/// Another process saving a newer session must not be undone by a stale one.
#[tokio::test]
async fn stale_process_keeps_newer_session_and_picks_it_up() {
    let (server, home, _client, auth) = setup().await;
    mock_session_and_profile(&server).await;
    auth.complete_login("req123").await.unwrap(); // our token: acc456

    // Another process logs in and overwrites the file with a new token.
    let path = home.path().join("session.json");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("acc456", "newer789");
    std::fs::write(&path, text).unwrap();

    // Our token is rejected; clearing must leave the newer session on disk.
    auth.clear().unwrap();
    assert!(path.exists());
    assert!(!auth.status().logged_in);

    auth.sync();
    assert!(auth.status().logged_in);
    assert!(std::fs::read_to_string(&path).unwrap().contains("newer789"));
}

#[tokio::test]
async fn clear_removes_own_session_file() {
    let (server, home, _client, auth) = setup().await;
    mock_session_and_profile(&server).await;
    auth.complete_login("req123").await.unwrap();
    auth.clear().unwrap();
    assert!(!home.path().join("session.json").exists());
}
