//! Desktop OAuth uses the system browser and an ephemeral loopback callback.
//! Tokens/configuration never cross IPC and are never included in error strings.
use crate::{
    network::{read_limited, Network},
    security::SecretStore,
};
use oauth2::{CsrfToken, PkceCodeChallenge};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{watch, Mutex},
};
use zeroize::{Zeroize, Zeroizing};

const READONLY: &str = "https://www.googleapis.com/auth/gmail.readonly";
const TOKEN_ENTRY: &str = "oauth.tokens.v1";
const RECONNECT: &str = "Google authorization expired or was revoked. Reconnect Gmail; downloaded mail remains available offline.";
const CONFIG_ERROR: &str = "Set PITHMAIL_OAUTH_CONFIG to your private Google Desktop-app client JSON (owner-only permissions, no symlink). Never put credentials in source control.";
const CALLBACK_PATH: &str = "/oauth/callback";

/// Classification survives OAuth refresh so sync can back off without new consent.
#[derive(Debug)]
pub enum AuthError {
    Reconnect(String),
    Retry {
        message: String,
        retry_after_secs: Option<u64>,
    },
    Local(String),
}

impl AuthError {
    pub fn into_message(self) -> String {
        match self {
            Self::Reconnect(message) | Self::Retry { message, .. } | Self::Local(message) => {
                message
            }
        }
    }
}

pub struct AuthService {
    vault: Arc<dyn SecretStore>,
    network: Network,
    config_path: Option<PathBuf>,
    refresh: Mutex<()>,
    connecting: Mutex<()>,
    cancel: watch::Sender<u64>,
}

#[derive(Deserialize)]
struct ConfigFile {
    installed: DesktopConfig,
}
#[derive(Deserialize)]
struct DesktopConfig {
    client_id: String,
    client_secret: Option<String>,
}
impl Drop for DesktopConfig {
    fn drop(&mut self) {
        self.client_id.zeroize();
        self.client_secret.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    client_id: String,
    scope: String,
}
impl Drop for StoredTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.client_id.zeroize();
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: Option<String>,
    scope: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

impl AuthService {
    pub fn new(
        vault: Arc<dyn SecretStore>,
        network: Network,
        config_path: Option<PathBuf>,
    ) -> Self {
        let (cancel, _) = watch::channel(0);
        Self {
            vault,
            network,
            config_path,
            refresh: Mutex::new(()),
            connecting: Mutex::new(()),
            cancel,
        }
    }

    pub async fn connected(&self) -> Result<bool, String> {
        Ok(self
            .load_tokens()
            .await
            .map_err(AuthError::into_message)?
            .is_some())
    }

    pub async fn cancel(&self) {
        self.cancel
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub async fn connect(&self) -> Result<(), String> {
        let _flow = self
            .connecting
            .try_lock()
            .map_err(|_| "A Google sign-in is already in progress.".to_string())?;
        let mut cancelled = self.cancel.subscribe();
        tokio::select! {
            biased;
            _ = cancelled.changed() => Err("Google sign-in cancelled.".into()),
            result = tokio::time::timeout(Duration::from_secs(180), self.connect_inner()) => {
                result.map_err(|_| "Google sign-in timed out. Retry from Settings.".to_string())?
            }
        }
    }

    async fn connect_inner(&self) -> Result<(), String> {
        let config = self.config().await?;
        // Fail before opening a browser if the credential store cannot be accessed.
        self.probe_vault().await?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| "Could not open a local Google sign-in callback.".to_string())?;
        let port = listener
            .local_addr()
            .map_err(|_| "Local sign-in callback unavailable.")?
            .port();
        let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let state = CsrfToken::new_random();
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
            .map_err(|_| "Invalid Google authorization endpoint.")?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("response_type", "code"),
            ("scope", READONLY),
            ("state", state.secret()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("include_granted_scopes", "false"),
        ]);
        tokio::task::spawn_blocking(move || {
            open::that(url.as_str()).map_err(|_| {
                "Could not open your system browser. Check the default browser configuration."
                    .to_string()
            })
        })
        .await
        .map_err(|_| "Could not open your system browser.")??;
        let code = receive_callback(&listener, port, state.secret()).await?;
        let mut form = vec![
            ("client_id".into(), config.client_id.clone()),
            ("code".into(), code.to_string()),
            ("code_verifier".into(), verifier.secret().clone()),
            ("grant_type".into(), "authorization_code".into()),
            ("redirect_uri".into(), redirect),
        ];
        if let Some(secret) = &config.client_secret {
            form.push(("client_secret".into(), secret.clone()));
        }
        let response = self.exchange(&form).await;
        form.zeroize();
        let response = response.map_err(AuthError::into_message)?;
        let tokens = validated_tokens(response, None, &config.client_id, now())?;
        let _refresh = self.refresh.lock().await;
        self.save_tokens(tokens).await
    }

    pub async fn access_token(&self) -> Result<Zeroizing<String>, AuthError> {
        // Single-flight reload after taking the lock prevents duplicate refreshes.
        let _refresh = self.refresh.lock().await;
        let previous = self
            .load_tokens()
            .await?
            .ok_or_else(|| AuthError::Reconnect(RECONNECT.into()))?;
        if previous.scope != READONLY {
            return Err(AuthError::Reconnect("Stored Google permissions do not match read-only Gmail. Reconnect with only gmail.readonly.".into()));
        }
        if previous.expires_at > now().saturating_add(60) {
            return Ok(Zeroizing::new(previous.access_token.clone()));
        }
        let config = self.config().await.map_err(AuthError::Local)?;
        if config.client_id != previous.client_id {
            return Err(AuthError::Reconnect(
                "The OAuth client changed. Restore its private configuration or reconnect Gmail."
                    .into(),
            ));
        }
        let mut form = vec![
            ("client_id".into(), config.client_id.clone()),
            ("refresh_token".into(), previous.refresh_token.clone()),
            ("grant_type".into(), "refresh_token".into()),
        ];
        if let Some(secret) = &config.client_secret {
            form.push(("client_secret".into(), secret.clone()));
        }
        let response = self.exchange(&form).await;
        form.zeroize();
        let tokens = validated_tokens(response?, Some(&previous), &config.client_id, now())
            .map_err(AuthError::Reconnect)?;
        let access = Zeroizing::new(tokens.access_token.clone());
        self.save_tokens(tokens).await.map_err(AuthError::Local)?;
        Ok(access)
    }

    /// A Gmail 401 invalidates the access token, not the retained refresh token.
    pub async fn invalidate_access_token(&self) -> Result<(), String> {
        let _refresh = self.refresh.lock().await;
        if let Some(mut tokens) = self.load_tokens().await.map_err(AuthError::into_message)? {
            tokens.expires_at = 0;
            self.save_tokens(tokens).await?;
        }
        Ok(())
    }

    async fn exchange(&self, form: &[(String, String)]) -> Result<TokenResponse, AuthError> {
        let response = self
            .network
            .token(form)
            .await
            .map_err(|_| AuthError::Retry {
                message: "Google authentication is unreachable. Synchronization will retry.".into(),
                retry_after_secs: None,
            })?;
        let status = response.status();
        let retry_after_secs = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| retry_after_seconds(value, SystemTime::now()));
        let bytes = Zeroizing::new(read_limited(response, 64 * 1024).await.map_err(|_| AuthError::Retry {
            message: "Google authentication response was interrupted or invalid. Retry synchronization.".into(),
            retry_after_secs,
        })?);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(AuthError::Retry {
                message: "Google authentication is temporarily unavailable or rate-limited. Synchronization will retry.".into(),
                retry_after_secs,
            });
        }
        if !status.is_success() {
            #[derive(Deserialize)]
            struct OAuthError {
                error: String,
            }
            let invalid = serde_json::from_slice::<OAuthError>(&bytes).is_ok_and(|error| {
                matches!(error.error.as_str(), "invalid_grant" | "invalid_token")
            });
            return Err(if invalid {
                AuthError::Reconnect(RECONNECT.into())
            } else {
                AuthError::Local(
                    "Google authentication failed. Verify your Desktop OAuth client and retry."
                        .into(),
                )
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            AuthError::Reconnect(
                "Google returned an invalid authentication response. Reconnect Gmail.".into(),
            )
        })
    }

    async fn config(&self) -> Result<DesktopConfig, String> {
        let path = self.config_path.clone().ok_or(CONFIG_ERROR)?;
        tokio::task::spawn_blocking(move || read_config(&path))
            .await
            .map_err(|_| CONFIG_ERROR.to_string())?
    }

    /// Consent must be able to repair damaged tokens, but never bypass a locked vault.
    async fn probe_vault(&self) -> Result<(), String> {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || vault.get(TOKEN_ENTRY).map(|_| ()))
            .await
            .map_err(|_| "Secure credential operation failed.".to_string())?
    }

    async fn load_tokens(&self) -> Result<Option<StoredTokens>, AuthError> {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || {
            vault
                .get(TOKEN_ENTRY)
                .map_err(AuthError::Local)?
                .map(|bytes| {
                    let tokens: StoredTokens = serde_json::from_slice(&bytes).map_err(|_| {
                        AuthError::Reconnect(
                            "Stored Google credentials are damaged. Reconnect Gmail.".into(),
                        )
                    })?;
                    if tokens.access_token.is_empty()
                        || tokens.refresh_token.is_empty()
                        || tokens.scope != READONLY
                    {
                        return Err(AuthError::Reconnect(
                            "Stored Google credentials are invalid. Reconnect Gmail.".into(),
                        ));
                    }
                    Ok(tokens)
                })
                .transpose()
        })
        .await
        .map_err(|_| AuthError::Local("Secure credential operation failed.".into()))?
    }

    async fn save_tokens(&self, tokens: StoredTokens) -> Result<(), String> {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = Zeroizing::new(
                serde_json::to_vec(&tokens)
                    .map_err(|_| "Could not encode secure credentials.".to_string())?,
            );
            vault.set(TOKEN_ENTRY, &bytes)
        })
        .await
        .map_err(|_| "Secure credential operation failed.".to_string())?
    }
}

fn retry_after_seconds(value: &str, timestamp: SystemTime) -> Option<u64> {
    value.trim().parse().ok().or_else(|| {
        httpdate::parse_http_date(value).ok().map(|date| {
            date.duration_since(timestamp)
                .map(|remaining| {
                    remaining
                        .as_secs()
                        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
                })
                .unwrap_or(0)
        })
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

fn validated_tokens(
    mut response: TokenResponse,
    previous: Option<&StoredTokens>,
    client_id: &str,
    timestamp: u64,
) -> Result<StoredTokens, String> {
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || response.access_token.is_empty()
        || response.access_token.len() > 32 * 1024
        || response.expires_in == 0
        || response.expires_in > 31_536_000
    {
        return Err("Google returned invalid token metadata.".into());
    }
    // Refresh responses may omit scope; only the previously verified scope survives.
    let scope = response
        .scope
        .as_deref()
        .or_else(|| previous.map(|tokens| tokens.scope.as_str()))
        .ok_or("Google did not confirm the granted Gmail scope.")?;
    let scopes: Vec<_> = scope.split_whitespace().collect();
    if scopes != [READONLY] {
        return Err(
            "Google granted unexpected permissions. Reconnect requesting only gmail.readonly."
                .into(),
        );
    }
    let refresh_token = response
        .refresh_token
        .take()
        .or_else(|| previous.map(|tokens| tokens.refresh_token.clone()))
        .ok_or("Google did not return offline access. Reconnect Gmail and grant consent.")?;
    if refresh_token.is_empty() || refresh_token.len() > 32 * 1024 {
        return Err("Google returned an invalid refresh token.".into());
    }
    Ok(StoredTokens {
        access_token: std::mem::take(&mut response.access_token),
        refresh_token,
        expires_at: timestamp.saturating_add(response.expires_in),
        client_id: client_id.into(),
        scope: READONLY.into(),
    })
}

fn read_config(path: &Path) -> Result<DesktopConfig, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| CONFIG_ERROR.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(CONFIG_ERROR.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CONFIG_ERROR.into());
        }
    }
    let file = File::open(path).map_err(|_| CONFIG_ERROR.to_string())?;
    let opened = file.metadata().map_err(|_| CONFIG_ERROR.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Detect replacement between permission/symlink checks and opening.
        if metadata.dev() != opened.dev()
            || metadata.ino() != opened.ino()
            || opened.permissions().mode() & 0o077 != 0
        {
            return Err(CONFIG_ERROR.into());
        }
    }
    if opened.len() > 64 * 1024 {
        return Err(CONFIG_ERROR.into());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CONFIG_ERROR.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err(CONFIG_ERROR.into());
    }
    let config: ConfigFile =
        serde_json::from_slice(&bytes).map_err(|_| CONFIG_ERROR.to_string())?;
    if !config
        .installed
        .client_id
        .ends_with(".apps.googleusercontent.com")
        || config.installed.client_id.len() > 1024
        || config
            .installed
            .client_secret
            .as_ref()
            .is_some_and(|secret| secret.len() > 4096)
    {
        return Err(CONFIG_ERROR.into());
    }
    Ok(config.installed)
}

enum Callback {
    Ignore,
    Denied,
    Code(Zeroizing<String>),
}

fn callback(request: &[u8], port: u16, expected_state: &str) -> Callback {
    let Ok(request) = std::str::from_utf8(request) else {
        return Callback::Ignore;
    };
    let mut lines = request.split("\r\n");
    let Some(first) = lines.next() else {
        return Callback::Ignore;
    };
    let mut words = first.split_whitespace();
    let (Some("GET"), Some(target), Some("HTTP/1.1"), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Callback::Ignore;
    };
    if !target.starts_with(&format!("{CALLBACK_PATH}?")) || target.contains('#') {
        return Callback::Ignore;
    }
    let hosts: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .collect();
    if hosts != [format!("127.0.0.1:{port}")] {
        return Callback::Ignore;
    }
    let Ok(url) = Url::parse(&format!("http://127.0.0.1:{port}{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != CALLBACK_PATH {
        return Callback::Ignore;
    }
    let mut pairs = HashMap::new();
    for (name, value) in url.query_pairs() {
        if pairs
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Callback::Ignore;
        }
    }
    let Some(state) = pairs.get("state") else {
        return Callback::Ignore;
    };
    if !equal_state(state.as_bytes(), expected_state.as_bytes()) {
        return Callback::Ignore;
    }
    if pairs.contains_key("error") {
        return Callback::Denied;
    }
    match pairs.remove("code") {
        Some(code) if !code.is_empty() && code.len() <= 4096 => {
            Callback::Code(Zeroizing::new(code))
        }
        _ => Callback::Ignore,
    }
}

fn equal_state(left: &[u8], right: &[u8]) -> bool {
    // Fixed-length, independently random state; compare every byte without early exit.
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

async fn receive_callback(
    listener: &TcpListener,
    port: u16,
    state: &str,
) -> Result<Zeroizing<String>, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| "Local Google callback failed.".to_string())?;
        let request = tokio::time::timeout(Duration::from_secs(3), async {
            let mut request = Zeroizing::new(Vec::new());
            let mut buffer = [0_u8; 1024];
            loop {
                let count = stream.read(&mut buffer).await.map_err(|_| ())?;
                if count == 0 || request.len() + count > 8192 {
                    return Err(());
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    return Ok(request);
                }
            }
        })
        .await;
        let parsed = match request {
            Ok(Ok(bytes)) => callback(&bytes, port, state),
            _ => Callback::Ignore,
        };
        let (status, body) = match &parsed {
            Callback::Ignore => ("400 Bad Request", "This sign-in callback was not accepted. Return to Pithmail or continue signing in."),
            Callback::Denied => ("200 OK", "Google sign-in was declined. You can close this tab and return to Pithmail."),
            Callback::Code(_) => ("200 OK", "Sign-in received. You can close this tab and return to Pithmail."),
        };
        let reply = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ =
            tokio::time::timeout(Duration::from_secs(1), stream.write_all(reply.as_bytes())).await;
        match parsed {
            Callback::Code(code) => return Ok(code),
            Callback::Denied => return Err("Google sign-in was declined.".into()),
            Callback::Ignore => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestSecrets(std::sync::Mutex<HashMap<String, Vec<u8>>>);
    impl SecretStore for TestSecrets {
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

    #[tokio::test]
    async fn damaged_tokens_can_be_repaired_only_after_valid_new_consent() {
        let vault = Arc::new(TestSecrets::default());
        vault.set(TOKEN_ENTRY, b"damaged-json").unwrap();
        let auth = AuthService::new(vault.clone(), Network::new().unwrap(), None);
        assert!(matches!(
            auth.access_token().await,
            Err(AuthError::Reconnect(_))
        ));
        // This is the same pre-browser check used by connect_inner. It proves
        // accessibility without making valid old credentials a consent prerequisite.
        auth.probe_vault().await.unwrap();
        let invalid = validated_tokens(
            response(Some("unexpected-scope"), Some("new-refresh")),
            None,
            "client",
            now(),
        );
        assert!(invalid.is_err());
        assert_eq!(
            vault.get(TOKEN_ENTRY).unwrap().unwrap().as_slice(),
            b"damaged-json"
        );
        let accepted = validated_tokens(
            response(Some(READONLY), Some("new-refresh")),
            None,
            "client",
            now(),
        )
        .unwrap();
        auth.save_tokens(accepted).await.unwrap();
        assert_eq!(auth.access_token().await.unwrap().as_str(), "access");
        assert_eq!(
            auth.load_tokens().await.unwrap().unwrap().refresh_token,
            "new-refresh"
        );
    }

    #[tokio::test]
    async fn consent_probe_does_not_bypass_a_locked_vault() {
        struct Locked;
        impl SecretStore for Locked {
            fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
                Err("Secure store is locked.".into())
            }
            fn set(&self, _: &str, _: &[u8]) -> Result<(), String> {
                panic!("locked store must not be written")
            }
        }
        let auth = AuthService::new(Arc::new(Locked), Network::new().unwrap(), None);
        assert_eq!(
            auth.probe_vault().await.unwrap_err(),
            "Secure store is locked."
        );
        assert!(matches!(
            auth.access_token().await,
            Err(AuthError::Local(_))
        ));
    }

    fn request(target: &str) -> Vec<u8> {
        format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n").into_bytes()
    }
    #[test]
    fn callback_requires_exact_path_host_state_and_unique_parameters() {
        assert!(matches!(
            callback(
                &request("/oauth/callback?state=expected&code=abc"),
                1234,
                "expected"
            ),
            Callback::Code(_)
        ));
        for target in [
            "/wrong?state=expected&code=abc",
            "/oauth/callback?state=wrong&code=abc",
            "/oauth/callback?state=expected&state=expected&code=abc",
            "/oauth/callback?code=abc",
            "/oauth/callback?state=expected&code=abc#fragment",
            "http://evil.test/oauth/callback?state=expected&code=abc",
        ] {
            assert!(matches!(
                callback(&request(target), 1234, "expected"),
                Callback::Ignore
            ));
        }
        assert!(matches!(
            callback(
                &request("/oauth/callback?state=expected&error=access_denied"),
                1234,
                "expected"
            ),
            Callback::Denied
        ));
        assert!(matches!(
            callback(
                b"GET /oauth/callback?state=expected&code=abc HTTP/1.1\r\nHost: evil.test\r\n\r\n",
                1234,
                "expected"
            ),
            Callback::Ignore
        ));
    }
    #[tokio::test]
    async fn loopback_ignores_unrelated_request_before_valid_callback() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = tokio::spawn(async move {
            for state in ["wrong", "expected"] {
                let mut stream =
                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                        .await
                        .unwrap();
                let text = format!("GET /oauth/callback?state={state}&code=accepted HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
                stream.write_all(text.as_bytes()).await.unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(if state == "wrong" {
                    b"HTTP/1.1 400"
                } else {
                    b"HTTP/1.1 200"
                }));
            }
        });
        let code = tokio::time::timeout(
            Duration::from_secs(5),
            receive_callback(&listener, port, "expected"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(code.as_str(), "accepted");
        client.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_access_refreshes_once_and_preserves_offline_token() {
        use std::os::unix::fs::PermissionsExt;
        use std::{collections::HashMap, sync::Mutex as StdMutex};
        #[derive(Default)]
        struct Memory(StdMutex<HashMap<String, Vec<u8>>>);
        impl SecretStore for Memory {
            fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .get(name)
                    .cloned()
                    .map(Zeroizing::new))
            }
            fn set(&self, name: &str, value: &[u8]) -> Result<(), String> {
                self.0.lock().unwrap().insert(name.into(), value.into());
                Ok(())
            }
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let network = Network::test_token_endpoint(
            Url::parse(&format!("http://127.0.0.1:{port}/token")).unwrap(),
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 2048];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                let text = std::str::from_utf8(&bytes).unwrap();
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|length| length.trim().parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= length {
                        assert!(body.contains("grant_type=refresh_token"));
                        assert!(body.contains("refresh_token=old-refresh"));
                        break;
                    }
                }
            }
            let body =
                r#"{"access_token":"renewed-access","token_type":"Bearer","expires_in":3600}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            // A second request would mean single-flight refresh failed.
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
        });
        let path = std::env::temp_dir().join(format!(
            "pithmail-refresh-config-{}-{port}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            br#"{"installed":{"client_id":"test.apps.googleusercontent.com"}}"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let vault = Arc::new(Memory::default());
        let previous = StoredTokens {
            access_token: "expired-access".into(),
            refresh_token: "old-refresh".into(),
            expires_at: 0,
            client_id: "test.apps.googleusercontent.com".into(),
            scope: READONLY.into(),
        };
        vault
            .set(TOKEN_ENTRY, &serde_json::to_vec(&previous).unwrap())
            .unwrap();
        let auth = Arc::new(AuthService::new(vault.clone(), network, Some(path.clone())));
        let mut callers = Vec::new();
        for _ in 0..8 {
            let auth = auth.clone();
            callers.push(tokio::spawn(
                async move { auth.access_token().await.unwrap() },
            ));
        }
        for caller in callers {
            assert_eq!(caller.await.unwrap().as_str(), "renewed-access");
        }
        server.await.unwrap();
        let persisted = auth.load_tokens().await.unwrap().unwrap();
        assert_eq!(persisted.refresh_token, "old-refresh");
        std::fs::remove_file(path).unwrap();
    }

    fn response(scope: Option<&str>, refresh: Option<&str>) -> TokenResponse {
        TokenResponse {
            access_token: "access".into(),
            token_type: "Bearer".into(),
            expires_in: 3600,
            refresh_token: refresh.map(str::to_owned),
            scope: scope.map(str::to_owned),
        }
    }
    #[test]
    fn refresh_preserves_refresh_token_and_rejects_broader_scopes() {
        let original = validated_tokens(
            response(Some(READONLY), Some("refresh")),
            None,
            "client",
            100,
        )
        .unwrap();
        let refreshed =
            validated_tokens(response(None, None), Some(&original), "client", 200).unwrap();
        assert_eq!(refreshed.refresh_token, "refresh");
        assert_eq!(refreshed.expires_at, 3800);
        assert!(validated_tokens(
            response(Some(&format!("{READONLY} openid")), Some("refresh")),
            None,
            "client",
            0
        )
        .is_err());
        assert!(validated_tokens(response(None, Some("refresh")), None, "client", 0).is_err());
        assert!(validated_tokens(response(Some(READONLY), None), None, "client", 0).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn config_refuses_shared_permissions_symlinks_and_web_clients() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = std::env::temp_dir().join(format!(
            "pithmail-oauth-config-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        std::fs::write(&path,br#"{"installed":{"client_id":"test.apps.googleusercontent.com","client_secret":"local-only"}}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_config(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_config(&path).is_ok());
        let link = dir.join("link.json");
        symlink(&path, &link).unwrap();
        assert!(read_config(&link).is_err());
        std::fs::write(
            &path,
            br#"{"web":{"client_id":"test.apps.googleusercontent.com"}}"#,
        )
        .unwrap();
        assert!(read_config(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
