use super::*;
use crate::security::SecretStore;
use std::{collections::HashMap, sync::Mutex as StdMutex, time::UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Secrets(StdMutex<HashMap<String, Vec<u8>>>);
impl SecretStore for Secrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(Zeroizing::new))
    }
    fn set(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(name.into(), bytes.into());
        Ok(())
    }
}

struct Reply {
    path: &'static str,
    status: u16,
    body: String,
    extra: &'static str,
    token: Option<&'static str>,
}
fn reply(path: &'static str, body: Value) -> Reply {
    Reply {
        path,
        status: 200,
        body: body.to_string(),
        extra: "",
        token: Some("old-token"),
    }
}
async fn mock(replies: Vec<Reply>) -> (Network, tokio::task::JoinHandle<Vec<Instant>>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint =
        reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let mut times = Vec::new();
        for reply in replies {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap();
            times.push(Instant::now());
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let length = stream.read(&mut buffer).await.unwrap();
                assert!(length > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..length]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&request);
            let first = headers.lines().next().unwrap();
            let method = if reply.path == "/token" {
                "POST"
            } else {
                "GET"
            };
            let mut words = first.split_whitespace();
            assert_eq!(words.next(), Some(method));
            assert_eq!(words.next().unwrap().trim_end_matches('?'), reply.path);
            assert_eq!(words.next(), Some("HTTP/1.1"));
            if let Some(token) = reply.token {
                assert!(headers
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {token}")));
            }
            let response = format!("HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}", reply.status, reply.body.len(), reply.extra, reply.body);
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        times
    });
    (Network::test_gmail_endpoint(endpoint), task)
}
fn auth(network: Network, config: Option<std::path::PathBuf>) -> Arc<AuthService> {
    auth_with_secrets(network, config).0
}
fn auth_with_secrets(
    network: Network,
    config: Option<std::path::PathBuf>,
) -> (Arc<AuthService>, Arc<Secrets>) {
    let secrets = Arc::new(Secrets::default());
    secrets.set("oauth.tokens.v1", &serde_json::to_vec(&serde_json::json!({
        "access_token":"old-token", "refresh_token":"offline-token", "expires_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+3600,
        "client_id":"test.apps.googleusercontent.com", "scope":"https://www.googleapis.com/auth/gmail.readonly"
    })).unwrap()).unwrap();
    (
        Arc::new(AuthService::new(secrets.clone(), network, config)),
        secrets,
    )
}
fn private_config() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "pithmail-refresh-error-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).unwrap();
    use std::io::Write;
    file.write_all(br#"{"installed":{"client_id":"test.apps.googleusercontent.com"}}"#)
        .unwrap();
    path
}

fn ok<T>(result: Result<T, ProviderError>) -> T {
    result.unwrap_or_else(|_| panic!("unexpected provider error"))
}

#[tokio::test]
async fn http_adapter_preserves_full_payload_all_mail_scope_and_metadata() {
    let original = serde_json::json!({"id":"a1","threadId":"t1","labelIds":["SENT"],"internalDate":"1","payload":{"partId":"0","mimeType":"text/plain","body":{"data":"aGVsbG8","size":5}},"unmodeled":"preserved"});
    let mut replies = vec![
        reply("/gmail/v1/users/me/profile", serde_json::json!({"emailAddress":"reader@example.test","historyId":"900"})),
        reply("/gmail/v1/users/me/messages?includeSpamTrash=true&maxResults=500&pageToken=next%2Bpage", serde_json::json!({"messages":[{"id":"a1","threadId":"t1"}]})),
        reply("/gmail/v1/users/me/messages/a1?format=full", original.clone()),
        reply("/gmail/v1/users/me/messages/a1/attachments/part_1", serde_json::json!({"data":"_wAB","size":3})),
        reply("/gmail/v1/users/me/history?startHistoryId=900&maxResults=500&pageToken=history", serde_json::json!({"historyId":"901","history":[]})),
        reply("/gmail/v1/users/me/labels", serde_json::json!({"labels":[{"id":"Label_1","name":"Research","type":"user","color":{"backgroundColor":"#ffffff"}}]})),
    ];
    // Preserve exact provider bytes, not a serialization of the modeled struct.
    replies[2].body = format!("  {}\n", original);
    let expected_json = replies[2].body.as_bytes().to_vec();
    let (network, server) = mock(replies).await;
    let client = GmailClient::new(auth(network.clone(), None), network);
    assert_eq!(ok(client.profile().await).history_id, "900");
    assert_eq!(
        ok(client.list(Some("next+page".into())).await)
            .messages
            .len(),
        1
    );
    let fetched = ok(client.message("a1".into()).await);
    assert_eq!(fetched.original_json, expected_json);
    assert_eq!(
        ok(client.attachment("a1".into(), "part_1".into()).await),
        vec![255, 0, 1]
    );
    assert_eq!(
        ok(client.history("900".into(), Some("history".into())).await).history_id,
        "901"
    );
    assert_eq!(ok(client.labels().await)[0]["name"], "Research");
    let times = server.await.unwrap();
    assert!(times
        .windows(2)
        .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(980)));
}

#[tokio::test]
async fn cloned_clients_share_request_pacing() {
    let (network, server) = mock(
        (0..3)
            .map(|_| {
                reply(
                    "/gmail/v1/users/me/profile",
                    serde_json::json!({"emailAddress":"reader@example.test","historyId":"1"}),
                )
            })
            .collect(),
    )
    .await;
    let client = GmailClient::new(auth(network.clone(), None), network);
    let a = client.clone();
    let b = client.clone();
    let (first, second, third) = tokio::join!(client.profile(), a.profile(), b.profile());
    assert!(first.is_ok() && second.is_ok() && third.is_ok());
    let times = server.await.unwrap();
    assert!(times
        .windows(2)
        .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(980)));
}

#[tokio::test]
async fn unauthorized_refreshes_once_and_second_unauthorized_requires_reconnect() {
    for succeeds in [true, false] {
        let mut denied = reply(
            "/gmail/v1/users/me/profile",
            serde_json::json!({"error":{"message":"private diagnostic"}}),
        );
        denied.status = 401;
        let mut refreshed = reply(
            "/token",
            serde_json::json!({"access_token":"new-token","token_type":"Bearer","expires_in":3600}),
        );
        refreshed.token = None;
        let mut final_reply = reply(
            "/gmail/v1/users/me/profile",
            if succeeds {
                serde_json::json!({"emailAddress":"reader@example.test","historyId":"1"})
            } else {
                serde_json::json!({"error":{}})
            },
        );
        final_reply.token = Some("new-token");
        if !succeeds {
            final_reply.status = 401;
        }
        let (network, server) = mock(vec![denied, refreshed, final_reply]).await;
        let path = std::env::temp_dir().join(format!(
            "pithmail-gmail-config-{}-{}.json",
            std::process::id(),
            u8::from(succeeds)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).unwrap();
        use std::io::Write;
        file.write_all(br#"{"installed":{"client_id":"test.apps.googleusercontent.com"}}"#)
            .unwrap();
        let client = GmailClient::new(auth(network.clone(), Some(path.clone())), network);
        let result = client.profile().await;
        if succeeds {
            assert!(result.is_ok());
        } else {
            assert!(matches!(
                result,
                Err(ProviderError {
                    kind: ProviderErrorKind::Reconnect,
                    ..
                })
            ));
        }
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn transient_refresh_statuses_retry_without_erasing_offline_credentials() {
    for (status, header, expected_delay) in [
        (503, "Retry-After: 73\r\n", 73),
        (429, "Retry-After: Thu, 01 Jan 2099 00:00:00 GMT\r\n", 1),
    ] {
        let mut denied = reply(
            "/gmail/v1/users/me/profile",
            serde_json::json!({"error":{}}),
        );
        denied.status = 401;
        let mut failure = reply(
            "/token",
            serde_json::json!({"error":"server_error","error_description":"private token diagnostic"}),
        );
        failure.status = status;
        failure.token = None;
        failure.extra = header;
        let (network, server) = mock(vec![denied, failure]).await;
        let path = private_config();
        let (auth, vault) = auth_with_secrets(network.clone(), Some(path.clone()));
        let client = GmailClient::new(auth.clone(), network);
        let error = client.profile().await.err().unwrap();
        assert!(error.kind == ProviderErrorKind::Retry);
        if status == 503 {
            assert_eq!(error.retry_after_secs, Some(expected_delay));
        } else {
            assert!(error
                .retry_after_secs
                .is_some_and(|delay| delay >= expected_delay));
        }
        assert!(!error.message.contains("private token diagnostic"));
        let stored: Value =
            serde_json::from_slice(&vault.get("oauth.tokens.v1").unwrap().unwrap()).unwrap();
        assert_eq!(stored["refresh_token"], "offline-token");
        assert_eq!(stored["access_token"], "old-token");
        assert!(auth.connected().await.unwrap());
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn refresh_transport_failure_retries_but_revoked_grant_requires_reconnect() {
    // A closed loopback port deterministically fails before any Gmail request.
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint =
        reqwest::Url::parse(&format!("http://{}/token", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let network = Network::test_token_endpoint(endpoint);
    let path = private_config();
    let (auth, vault) = auth_with_secrets(network.clone(), Some(path.clone()));
    auth.invalidate_access_token().await.unwrap();
    let before = vault.get("oauth.tokens.v1").unwrap().unwrap().to_vec();
    let client = GmailClient::new(auth, network);
    let error = client.profile().await.err().unwrap();
    assert!(error.kind == ProviderErrorKind::Retry);
    assert_eq!(error.retry_after_secs, None);
    assert_eq!(
        vault.get("oauth.tokens.v1").unwrap().unwrap().as_slice(),
        before.as_slice()
    );
    std::fs::remove_file(path).unwrap();

    let mut revoked = reply("/token", serde_json::json!({"error":"invalid_grant"}));
    revoked.status = 400;
    revoked.token = None;
    let (network, server) = mock(vec![revoked]).await;
    let path = private_config();
    let (auth, vault) = auth_with_secrets(network.clone(), Some(path.clone()));
    auth.invalidate_access_token().await.unwrap();
    let client = GmailClient::new(auth, network);
    assert!(matches!(
        client.profile().await,
        Err(ProviderError {
            kind: ProviderErrorKind::Reconnect,
            ..
        })
    ));
    let stored: Value =
        serde_json::from_slice(&vault.get("oauth.tokens.v1").unwrap().unwrap()).unwrap();
    assert_eq!(stored["refresh_token"], "offline-token");
    server.await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn local_refresh_configuration_failure_is_not_reported_as_revoked_consent() {
    let network = Network::new().unwrap();
    let auth = auth(network.clone(), None);
    auth.invalidate_access_token().await.unwrap();
    let client = GmailClient::new(auth, network);
    let error = client.profile().await.err().unwrap();
    assert!(error.kind == ProviderErrorKind::Permanent);
    assert!(error.message.contains("PITHMAIL_OAUTH_CONFIG"));
    assert!(!error.message.contains("expired or revoked"));
}

#[test]
fn status_classification_is_contextual_and_errors_do_not_echo_provider_content() {
    for (status, body, context, expected) in [
        (404, "{}", Context::Ordinary, ProviderErrorKind::NotFound),
        (
            404,
            "{}",
            Context::History,
            ProviderErrorKind::HistoryExpired,
        ),
        (
            400,
            r#"{"error":{"message":"Invalid pageToken"}}"#,
            Context::Listing { has_page: true },
            ProviderErrorKind::HistoryExpired,
        ),
        (
            400,
            r#"{"error":{"message":"Invalid pageToken"}}"#,
            Context::Listing { has_page: false },
            ProviderErrorKind::Permanent,
        ),
        (
            400,
            r#"{"error":{"message":"bad unrelated argument"}}"#,
            Context::Listing { has_page: true },
            ProviderErrorKind::Permanent,
        ),
        (
            403,
            r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#,
            Context::Ordinary,
            ProviderErrorKind::Retry,
        ),
        (
            403,
            r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
            Context::Ordinary,
            ProviderErrorKind::Retry,
        ),
        (
            403,
            r#"{"error":{"errors":[{"reason":"domainPolicy"}]}}"#,
            Context::Ordinary,
            ProviderErrorKind::Permanent,
        ),
        (
            429,
            "private diagnostic",
            Context::Ordinary,
            ProviderErrorKind::Retry,
        ),
        (
            503,
            "private diagnostic",
            Context::Ordinary,
            ProviderErrorKind::Retry,
        ),
        (
            401,
            "private diagnostic",
            Context::Ordinary,
            ProviderErrorKind::Reconnect,
        ),
    ] {
        let error = classify_error(
            StatusCode::from_u16(status).unwrap(),
            body.as_bytes(),
            Some(37),
            context,
        );
        assert!(error.kind == expected, "status {status}");
        assert_eq!(error.retry_after_secs, Some(37));
        assert!(!error.message.contains("private diagnostic"));
    }
    let now = UNIX_EPOCH + Duration::from_secs(1000);
    assert_eq!(retry_after_seconds("37", now), Some(37));
    assert_eq!(
        retry_after_seconds(&httpdate::fmt_http_date(now + Duration::from_secs(60)), now),
        Some(60)
    );
    assert_eq!(
        retry_after_seconds(&httpdate::fmt_http_date(now - Duration::from_secs(60)), now),
        Some(0)
    );
    assert_eq!(retry_after_seconds("invalid", now), None);
}

#[tokio::test]
async fn transient_http_errors_preserve_retry_after_and_malformed_parts_fail_closed() {
    let mut limited = reply(
        "/gmail/v1/users/me/messages?includeSpamTrash=true&maxResults=500",
        serde_json::json!({"error":{}}),
    );
    limited.status = 429;
    limited.extra = "Retry-After: 123\r\n";
    let (network, server) = mock(vec![
        limited,
        reply(
            "/gmail/v1/users/me/messages/a1/attachments/p1",
            serde_json::json!({"data":"@@@","size":2}),
        ),
    ])
    .await;
    let client = GmailClient::new(auth(network.clone(), None), network);
    let error = client.list(None).await.err().unwrap();
    assert!(error.kind == ProviderErrorKind::Retry);
    assert_eq!(error.retry_after_secs, Some(123));
    assert!(matches!(
        client.attachment("a1".into(), "p1".into()).await,
        Err(ProviderError {
            kind: ProviderErrorKind::Permanent,
            ..
        })
    ));
    server.await.unwrap();
    for id in ["", "../escape", "a?query=1", "a/b"] {
        assert!(validate_id(id).is_err());
    }
}
