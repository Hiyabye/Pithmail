use mail_parser::{MessageParser, MimeHeaders};

pub(crate) struct ProviderMessage {
    pub id: &'static str,
    pub thread_id: &'static str,
    pub labels: &'static [&'static str],
    pub raw: &'static [u8],
}

/// Provider metadata is separate from MIME: threading and labels are provider-owned.
pub(crate) trait MailProvider {
    fn messages(&self) -> Vec<ProviderMessage>;
}

pub(crate) struct FixtureProvider;

impl MailProvider for FixtureProvider {
    fn messages(&self) -> Vec<ProviderMessage> {
        vec![
            ProviderMessage {
                id: "planning-1",
                thread_id: "planning",
                labels: &["INBOX", "IMPORTANT"],
                raw: include_bytes!("../../fixtures/planning.eml"),
            },
            ProviderMessage {
                id: "planning-2",
                thread_id: "planning",
                labels: &["SENT"],
                raw: include_bytes!("../../fixtures/reply.eml"),
            },
            ProviderMessage {
                id: "korean",
                thread_id: "korean",
                labels: &["INBOX", "IMPORTANT"],
                raw: include_bytes!("../../fixtures/korean.eml"),
            },
            ProviderMessage {
                id: "library",
                thread_id: "library",
                labels: &["RECEIPTS"],
                raw: include_bytes!("../../fixtures/archive.eml"),
            },
            ProviderMessage {
                id: "spam",
                thread_id: "spam",
                labels: &["SPAM"],
                raw: include_bytes!("../../fixtures/spam.eml"),
            },
            ProviderMessage {
                id: "trash",
                thread_id: "trash",
                labels: &["TRASH"],
                raw: include_bytes!("../../fixtures/trash.eml"),
            },
        ]
    }
}

pub(crate) struct ParsedAttachment {
    pub name: String,
    pub content: Vec<u8>,
}

pub(crate) struct ParsedMail {
    pub subject: String,
    pub sender: String,
    pub received_at: i64,
    pub body: String,
    pub attachments: Vec<ParsedAttachment>,
}

pub(crate) fn parse(raw: &[u8]) -> Result<ParsedMail, String> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "Message has no readable MIME structure".to_string())?;
    let sender = message
        .from()
        .and_then(|addresses| addresses.first())
        .and_then(|address| address.address())
        .ok_or_else(|| "Message is missing a sender address".to_string())?
        .to_string();
    let received_at = message
        .date()
        .ok_or_else(|| "Message is missing a valid date".to_string())?
        .to_timestamp();
    let body = message
        .body_text(0)
        .map(|text| text.into_owned())
        .unwrap_or_default();
    let attachments = message
        .attachments()
        .map(|part| ParsedAttachment {
            name: part
                .attachment_name()
                .unwrap_or("unnamed attachment")
                .to_string(),
            content: part.contents().to_vec(),
        })
        .collect();
    Ok(ParsedMail {
        subject: message.subject().unwrap_or("(No subject)").to_string(),
        sender,
        received_at,
        body,
        attachments,
    })
}
