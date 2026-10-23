//! Read-only Gmail HTTP adapter. All transport passes through the fixed-origin policy.
use crate::{
    auth::{AuthError, AuthService},
    network::{read_limited, Network},
    provider::{
        FetchedMessage, GmailMessage, GmailSource, HistoryPage, ListPage, Profile, ProviderError,
        ProviderErrorKind,
    },
};
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{sync::Mutex, time::Instant};

const ROOT: &str = "/gmail/v1/users/me";
const MIB: usize = 1024 * 1024;
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct GmailClient {
    auth: Arc<AuthService>,
    network: Network,
    last_request: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone, Copy)]
enum Context {
    Ordinary,
    History,
    Listing { has_page: bool },
}

impl GmailClient {
    pub fn new(auth: Arc<AuthService>, network: Network) -> Self {
        Self {
            auth,
            network,
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    async fn pace(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + REQUEST_INTERVAL).await;
        }
        *last = Some(Instant::now());
    }

    async fn request(
        &self,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
        context: Context,
    ) -> Result<Vec<u8>, ProviderError> {
        for attempt in 0..2 {
            let token = self.auth.access_token().await.map_err(auth_failure)?;
            self.pace().await;
            let response = self.network.gmail(path, query, &token).await.map_err(|_| {
                retry("Gmail is unreachable. Check your connection; synchronization will retry.")
            })?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && attempt == 0 {
                self.auth
                    .invalidate_access_token()
                    .await
                    .map_err(|message| ProviderError::permanent(&message))?;
                continue;
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| retry_after_seconds(value, SystemTime::now()));
            let bytes = read_limited(response, if status.is_success() { limit } else { MIB }).await.map_err(|_| retry("Gmail response was interrupted or exceeded its size limit. Retry synchronization."))?;
            if status.is_success() {
                return Ok(bytes);
            }
            return Err(classify_error(status, &bytes, retry_after, context));
        }
        Err(reconnect())
    }

    async fn json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
        context: Context,
    ) -> Result<T, ProviderError> {
        let bytes = self.request(path, query, limit, context).await?;
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::permanent("Gmail returned malformed data. Retry synchronization; local mail remains available."))
    }
}

impl GmailSource for GmailClient {
    async fn profile(&self) -> Result<Profile, ProviderError> {
        let profile: Profile = self
            .json(&format!("{ROOT}/profile"), &[], MIB, Context::Ordinary)
            .await?;
        if profile.email_address.is_empty() || profile.history_id.is_empty() {
            return Err(ProviderError::permanent(
                "Gmail returned incomplete account metadata.",
            ));
        }
        Ok(profile)
    }

    async fn list(&self, page: Option<String>) -> Result<ListPage, ProviderError> {
        let has_page = page.as_ref().is_some_and(|value| !value.is_empty());
        let mut query = vec![
            ("includeSpamTrash", "true".into()),
            ("maxResults", "500".into()),
        ];
        if let Some(page) = page {
            query.push(("pageToken", page));
        }
        self.json(
            &format!("{ROOT}/messages"),
            &query,
            16 * MIB,
            Context::Listing { has_page },
        )
        .await
    }

    async fn message(&self, id: String) -> Result<FetchedMessage, ProviderError> {
        validate_id(&id)?;
        let original_json = self
            .request(
                &format!("{ROOT}/messages/{id}"),
                &[("format", "full".into())],
                64 * MIB,
                Context::Ordinary,
            )
            .await?;
        let message: GmailMessage = serde_json::from_slice(&original_json)
            .map_err(|_| ProviderError::permanent("Gmail returned a malformed message."))?;
        if message.id != id || message.thread_id.is_empty() {
            return Err(ProviderError::permanent(
                "Gmail returned inconsistent message identifiers.",
            ));
        }
        Ok(FetchedMessage {
            message,
            original_json,
        })
    }

    async fn attachment(
        &self,
        message: String,
        attachment: String,
    ) -> Result<Vec<u8>, ProviderError> {
        validate_id(&message)?;
        validate_id(&attachment)?;
        #[derive(Deserialize)]
        struct Attachment {
            data: String,
            size: u64,
        }
        let part: Attachment = self
            .json(
                &format!("{ROOT}/messages/{message}/attachments/{attachment}"),
                &[],
                128 * MIB,
                Context::Ordinary,
            )
            .await?;
        let bytes = URL_SAFE_NO_PAD
            .decode(&part.data)
            .or_else(|_| URL_SAFE.decode(&part.data))
            .map_err(|_| ProviderError::permanent("Gmail returned invalid attachment encoding."))?;
        if bytes.len() as u64 != part.size {
            return Err(ProviderError::permanent(
                "Gmail returned an incomplete attachment. Retry synchronization.",
            ));
        }
        Ok(bytes)
    }

    async fn history(
        &self,
        cursor: String,
        page: Option<String>,
    ) -> Result<HistoryPage, ProviderError> {
        if cursor.is_empty()
            || cursor.len() > 128
            || !cursor.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ProviderError::permanent(
                "The Gmail history checkpoint is invalid. Reconcile the mailbox.",
            ));
        }
        let mut query = vec![("startHistoryId", cursor), ("maxResults", "500".into())];
        if let Some(page) = page {
            query.push(("pageToken", page));
        }
        self.json(
            &format!("{ROOT}/history"),
            &query,
            16 * MIB,
            Context::History,
        )
        .await
    }

    async fn labels(&self) -> Result<Vec<Value>, ProviderError> {
        #[derive(Deserialize)]
        struct Labels {
            #[serde(default)]
            labels: Vec<Value>,
        }
        let response: Labels = self
            .json(&format!("{ROOT}/labels"), &[], 4 * MIB, Context::Ordinary)
            .await?;
        Ok(response.labels)
    }
}

fn validate_id(id: &str) -> Result<(), ProviderError> {
    if id.is_empty()
        || id.len() > 4096
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(ProviderError::permanent(
            "Gmail returned an invalid provider identifier.",
        ));
    }
    Ok(())
}

fn auth_failure(error: AuthError) -> ProviderError {
    match error {
        AuthError::Reconnect(message) => ProviderError {
            kind: ProviderErrorKind::Reconnect,
            retry_after_secs: None,
            message,
        },
        AuthError::Retry {
            message,
            retry_after_secs,
        } => ProviderError {
            kind: ProviderErrorKind::Retry,
            retry_after_secs,
            message,
        },
        AuthError::Local(message) => ProviderError::permanent(&message),
    }
}

fn reconnect() -> ProviderError {
    ProviderError { kind: ProviderErrorKind::Reconnect, retry_after_secs: None, message: "Gmail authorization is unavailable or expired. Reconnect Gmail; downloaded mail remains available offline.".into() }
}
fn retry(message: &str) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Retry,
        retry_after_secs: None,
        message: message.into(),
    }
}

fn retry_after_seconds(value: &str, now: SystemTime) -> Option<u64> {
    value.trim().parse().ok().or_else(|| {
        httpdate::parse_http_date(value).ok().map(|date| {
            date.duration_since(now)
                .map(|remaining| {
                    remaining
                        .as_secs()
                        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
                })
                .unwrap_or(0)
        })
    })
}

fn classify_error(
    status: StatusCode,
    bytes: &[u8],
    retry_after: Option<u64>,
    context: Context,
) -> ProviderError {
    let error: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let reasons: Vec<&str> = error
        .pointer("/error/errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.get("reason").and_then(Value::as_str))
        .collect();
    let message = error
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let invalid_page = reasons
        .iter()
        .any(|reason| matches!(*reason, "invalidPageToken" | "pageTokenExpired"))
        || ((message.contains("pagetoken") || message.contains("page token"))
            && (message.contains("invalid") || message.contains("expired")));
    let kind = if status == StatusCode::UNAUTHORIZED {
        ProviderErrorKind::Reconnect
    } else if status == StatusCode::NOT_FOUND {
        if matches!(context, Context::History) {
            ProviderErrorKind::HistoryExpired
        } else {
            ProviderErrorKind::NotFound
        }
    } else if status == StatusCode::BAD_REQUEST
        && matches!(context, Context::Listing { has_page: true })
        && invalid_page
    {
        ProviderErrorKind::HistoryExpired
    } else if status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (status == StatusCode::FORBIDDEN
            && reasons
                .iter()
                .any(|reason| matches!(*reason, "rateLimitExceeded" | "userRateLimitExceeded")))
    {
        ProviderErrorKind::Retry
    } else {
        ProviderErrorKind::Permanent
    };
    let message = match kind {
        ProviderErrorKind::Reconnect => reconnect().message,
        ProviderErrorKind::NotFound => "This item is no longer available from Gmail.".into(),
        ProviderErrorKind::HistoryExpired => {
            "Gmail's synchronization checkpoint expired. Reconciling the mailbox.".into()
        }
        ProviderErrorKind::Retry => {
            "Gmail is temporarily unavailable or rate-limited. Synchronization will retry.".into()
        }
        ProviderErrorKind::Permanent => {
            "Gmail refused this request. Check API access and permissions, then retry.".into()
        }
    };
    ProviderError {
        kind,
        retry_after_secs: retry_after,
        message,
    }
}

#[cfg(test)]
#[path = "gmail_tests.rs"]
mod tests;
