//! Provider-owned Gmail wire contracts. HTTP and parsing stay outside presentation.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, future::Future};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    pub history_id: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub name: String,
    pub value: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartBody {
    pub attachment_id: Option<String>,
    #[serde(default)]
    pub size: u64,
    pub data: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MimePart {
    #[serde(default)]
    pub part_id: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub body: PartBody,
    #[serde(default)]
    pub parts: Vec<MimePart>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    pub internal_date: String,
    pub payload: MimePart,
}
#[derive(Clone)]
pub struct FetchedMessage {
    pub message: GmailMessage,
    pub original_json: Vec<u8>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub message: MessageRef,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelChange {
    pub message: MessageRef,
    #[serde(default)]
    pub label_ids: Vec<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_added: Vec<LabelChange>,
    #[serde(default)]
    pub labels_removed: Vec<LabelChange>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    pub next_page_token: Option<String>,
    pub history_id: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    NotFound,
    HistoryExpired,
    Reconnect,
    Retry,
    Permanent,
}
#[derive(Clone)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub retry_after_secs: Option<u64>,
    pub message: String,
}
impl ProviderError {
    pub fn permanent(message: &str) -> Self {
        Self {
            kind: ProviderErrorKind::Permanent,
            retry_after_secs: None,
            message: message.into(),
        }
    }
}

pub trait GmailSource: Send + Sync {
    fn labels(&self) -> impl Future<Output = Result<Vec<serde_json::Value>, ProviderError>> + Send {
        async { Ok(Vec::new()) }
    }
    fn profile(&self) -> impl Future<Output = Result<Profile, ProviderError>> + Send;
    fn list(
        &self,
        page: Option<String>,
    ) -> impl Future<Output = Result<ListPage, ProviderError>> + Send;
    fn message(
        &self,
        id: String,
    ) -> impl Future<Output = Result<FetchedMessage, ProviderError>> + Send;
    fn attachment(
        &self,
        message: String,
        attachment: String,
    ) -> impl Future<Output = Result<Vec<u8>, ProviderError>> + Send;
    fn history(
        &self,
        cursor: String,
        page: Option<String>,
    ) -> impl Future<Output = Result<HistoryPage, ProviderError>> + Send;
}

pub struct NormalizedPart {
    pub part_id: String,
    pub attachment_id: Option<String>,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub is_body: bool,
    pub content: Option<Vec<u8>>,
}
pub struct NormalizedMessage {
    pub subject: String,
    pub sender: String,
    pub received_at: i64,
    pub body: String,
    pub parts: Vec<NormalizedPart>,
}
pub type ExternalParts = HashMap<String, Vec<u8>>;
