//! Gmail FULL contains transfer-decoded MIME bytes. Reconstruct only a parser
//! input for charset/header/HTML normalization; preserve original provider JSON separately.
use std::collections::HashSet;

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use mail_parser::MessageParser;

use crate::provider::{ExternalParts, GmailMessage, MimePart, NormalizedMessage, NormalizedPart};

const INVALID: &str = "Gmail returned an invalid MIME structure. Retry synchronization.";
const MAX_PART_SIZE: u64 = 128 * 1024 * 1024;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_PARTS: usize = 4096;

pub fn normalize(
    message: &GmailMessage,
    external: &ExternalParts,
) -> Result<NormalizedMessage, String> {
    if message.id.is_empty() || message.thread_id.is_empty() {
        return Err(INVALID.into());
    }
    let received_at = message
        .internal_date
        .parse::<i64>()
        .map_err(|_| INVALID.to_string())?
        / 1000;
    let headers = &message.payload.headers;
    let subject = header(headers, "Subject").unwrap_or("(No subject)");
    let sender = header(headers, "From").unwrap_or("(Unknown sender)");
    let root_input = format!(
        "Subject: {}\r\nFrom: {}\r\n\r\n",
        safe_header(subject)?,
        safe_header(sender)?
    );
    let root = MessageParser::default()
        .parse(root_input.as_bytes())
        .ok_or(INVALID)?;
    let subject = root.subject().unwrap_or("(No subject)").to_string();
    let sender = root
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .unwrap_or(sender)
        .to_string();
    let mut context = Context {
        external,
        parts: Vec::new(),
        ids: HashSet::new(),
        total: 0,
    };
    let body = context.visit(&message.payload, 0, false)?;
    Ok(NormalizedMessage {
        subject,
        sender,
        received_at,
        body,
        parts: context.parts,
    })
}

fn header<'a>(headers: &'a [crate::provider::Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn safe_header(value: &str) -> Result<String, String> {
    if value.len() > 64 * 1024 || value.contains('\0') {
        return Err(INVALID.into());
    }
    // Folded headers may contain line endings; they must never inject extra parser headers.
    Ok(value.replace(['\r', '\n'], " "))
}

struct Context<'a> {
    external: &'a ExternalParts,
    parts: Vec<NormalizedPart>,
    ids: HashSet<String>,
    total: u64,
}

impl Context<'_> {
    fn visit(
        &mut self,
        part: &MimePart,
        depth: usize,
        attached_parent: bool,
    ) -> Result<String, String> {
        if depth > 32 || self.ids.len() >= MAX_PARTS || !self.ids.insert(part.part_id.clone()) {
            return Err(INVALID.into());
        }
        if part.body.size > MAX_PART_SIZE
            || part.mime_type.len() > 256
            || part.filename.len() > 64 * 1024
        {
            return Err(INVALID.into());
        }
        let disposition = header(&part.headers, "Content-Disposition").unwrap_or("");
        let attached = attached_parent
            || !part.filename.is_empty()
            || disposition
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("attachment");
        let mime = part.mime_type.to_ascii_lowercase();
        let is_body = !attached && matches!(mime.as_str(), "text/plain" | "text/html");
        let content = if let Some(data) = &part.body.data {
            if data.len() as u64 > MAX_PART_SIZE * 4 / 3 + 8 {
                return Err(INVALID.into());
            }
            Some(
                URL_SAFE_NO_PAD
                    .decode(data.trim_end_matches('='))
                    .map_err(|_| INVALID.to_string())?,
            )
        } else if part.body.attachment_id.is_some() {
            self.external.get(&part.part_id).cloned()
        } else if part.body.size == 0 {
            Some(Vec::new())
        } else {
            return Err(INVALID.into());
        };
        if let Some(bytes) = &content {
            if bytes.len() as u64 != part.body.size {
                return Err(
                    "Gmail MIME part size did not match its contents. Retry synchronization."
                        .into(),
                );
            }
            self.total = self.total.checked_add(bytes.len() as u64).ok_or(INVALID)?;
            if self.total > MAX_TOTAL {
                return Err(INVALID.into());
            }
        }
        let is_container = mime.starts_with("multipart/");
        // Inventory every externally stored part, including body parts and containers.
        if !is_container || part.body.attachment_id.is_some() || part.body.size > 0 {
            self.parts.push(NormalizedPart {
                part_id: part.part_id.clone(),
                attachment_id: part.body.attachment_id.clone(),
                name: if part.filename.is_empty() {
                    "unnamed attachment".into()
                } else {
                    part.filename.clone()
                },
                mime_type: mime.clone(),
                size: part.body.size,
                is_body,
                content: content.clone(),
            });
        }
        if is_container {
            let mut bodies = Vec::new();
            let mut plain = Vec::new();
            for child in &part.parts {
                let text = self.visit(child, depth + 1, attached)?;
                if !text.is_empty() {
                    if child.mime_type.eq_ignore_ascii_case("text/plain") {
                        plain.push(text.clone());
                    }
                    bodies.push(text);
                }
            }
            if attached {
                return Ok(String::new());
            }
            if mime == "multipart/alternative" {
                return Ok(plain
                    .into_iter()
                    .next()
                    .or_else(|| bodies.into_iter().next())
                    .unwrap_or_default());
            }
            return Ok(bodies.join("\n\n"));
        }
        if !part.parts.is_empty() {
            // Nested RFC messages remain attachments, but their external parts still count.
            for child in &part.parts {
                self.visit(child, depth + 1, true)?;
            }
        }
        if !is_body {
            return Ok(String::new());
        }
        let Some(content) = content else {
            return Ok(String::new());
        };
        let content_type = header(&part.headers, "Content-Type").unwrap_or(&part.mime_type);
        // Gmail's bytes are already transfer-decoded. Standardize transfer encoding
        // rather than copying the provider's original quoted-printable/base64 header.
        let input = format!(
            "MIME-Version: 1.0\r\nContent-Type: {}\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            safe_header(content_type)?,
            STANDARD.encode(content)
        );
        let parsed = MessageParser::default()
            .parse(input.as_bytes())
            .ok_or(INVALID)?;
        Ok(parsed
            .body_text(0)
            .map(|b| b.into_owned())
            .unwrap_or_default())
    }
}
