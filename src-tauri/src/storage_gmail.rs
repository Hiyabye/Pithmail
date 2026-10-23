use super::*;
use crate::{
    gmail_mime,
    provider::{ExternalParts, FetchedMessage, GmailMessage},
};
use zeroize::Zeroizing;

const GMAIL_DB_ERROR: &str =
    "Encrypted mail storage failed. Keep the database and OS key; retry or restart Pithmail.";

impl Store {
    pub fn open_encrypted(path: &Path, key: &[u8; 32]) -> Result<Self, String> {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Refusing to open a linked real-mail database.".into());
        }
        let mut connection = Connection::open(path).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        let version: String = connection
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .map_err(|_| {
                "SQLCipher is unavailable. Real-mail storage remains locked.".to_string()
            })?;
        if version.is_empty() {
            return Err("SQLCipher is unavailable. Real-mail storage remains locked.".into());
        }
        let mut pragma = Zeroizing::new(String::from("PRAGMA key = \"x'"));
        use std::fmt::Write;
        for byte in key {
            write!(&mut *pragma, "{byte:02x}").map_err(|_| GMAIL_DB_ERROR.to_string())?;
        }
        pragma.push_str("'\";");
        connection
            .execute_batch(&pragma)
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, u32>(0))
            .map_err(|_| "Cannot unlock mail storage. Check the OS credential store; do not replace or delete its key.".to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY; PRAGMA cipher_memory_security=ON;")
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        match schema {
            0 => {
                let count: u32 = connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0)).map_err(|_| GMAIL_DB_ERROR.to_string())?;
                if count != 0 {
                    return Err("Refusing to initialize an unrelated real-mail database.".into());
                }
                let tx = connection
                    .transaction()
                    .map_err(|_| GMAIL_DB_ERROR.to_string())?;
                tx.execute_batch(include_str!("../migrations/002_gmail.sql"))
                    .map_err(|_| GMAIL_DB_ERROR.to_string())?;
                tx.commit().map_err(|_| GMAIL_DB_ERROR.to_string())?;
            }
            2 => {}
            _ => {
                return Err("This real-mail database requires another application version.".into())
            }
        }
        let kind: String = connection
            .query_row("SELECT kind FROM database_kind", [], |r| r.get(0))
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        if kind != "real-gmail" {
            return Err("This is not an encrypted Gmail database.".into());
        }
        connection
            .execute_batch(include_str!("../migrations/003_sync.sql"))
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        }
        Ok(Self {
            connection,
            is_gmail: true,
        })
    }

    pub fn account_email(&self) -> Result<Option<String>, String> {
        if !self.is_gmail {
            return Ok(None);
        }
        self.connection
            .query_row("SELECT address FROM accounts WHERE id='gmail'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|_| GMAIL_DB_ERROR.into())
    }

    pub fn set_account(&mut self, email: &str) -> Result<(), String> {
        self.require_gmail()?;
        if email.len() > 320 || !email.contains('@') || email.contains(['\r', '\n', '\0']) {
            return Err("Gmail returned an invalid account address.".into());
        }
        if self
            .account_email()?
            .is_some_and(|existing| !existing.eq_ignore_ascii_case(email))
        {
            return Err("This encrypted mailbox belongs to another Gmail account. Reconnect the original account.".into());
        }
        self.connection.execute("INSERT INTO accounts(id,provider,address) VALUES('gmail','gmail',?1) ON CONFLICT(id) DO UPDATE SET address=excluded.address", [email]).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        Ok(())
    }

    pub fn put_gmail(&mut self, fetched: &FetchedMessage) -> Result<(), String> {
        self.require_gmail()?;
        if fetched.original_json.len() > 128 * 1024 * 1024 {
            return Err("Gmail message exceeds the supported safety limit.".into());
        }
        let original: GmailMessage = serde_json::from_slice(&fetched.original_json)
            .map_err(|_| "Gmail returned invalid message data.".to_string())?;
        if original.id != fetched.message.id || original.thread_id != fetched.message.thread_id {
            return Err("Gmail message identifiers did not match.".into());
        }
        let message = &original;
        let id = format!("gmail:{}", message.id);
        let mut existing = ExternalParts::new();
        {
            let mut stmt = self.connection.prepare("SELECT part_id,attachment_id,expected_size,content FROM gmail_parts WHERE message_id=?1 AND attachment_id IS NOT NULL AND content IS NOT NULL").map_err(|_| GMAIL_DB_ERROR.to_string())?;
            let rows = stmt
                .query_map([&id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? as u64,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                })
                .map_err(|_| GMAIL_DB_ERROR.to_string())?;
            for row in rows {
                let (part_id, attachment_id, size, content) =
                    row.map_err(|_| GMAIL_DB_ERROR.to_string())?;
                if same_external_part(&message.payload, &part_id, &attachment_id, size) {
                    existing.insert(part_id, content);
                }
            }
        }
        let normalized = gmail_mime::normalize(message, &existing)?;
        let thread_id = format!("gmail:{}", message.thread_id);
        let snippet: String = normalized
            .body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        let tx = self
            .connection
            .transaction()
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO threads(id,account_id,provider_id) VALUES(?1,'gmail',?2)",
            params![thread_id, message.thread_id],
        )
        .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute("INSERT INTO messages(id,account_id,provider_id,thread_id,subject,sender,received_at,body,snippet) VALUES(?1,'gmail',?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(account_id,provider_id) DO UPDATE SET thread_id=excluded.thread_id,subject=excluded.subject,sender=excluded.sender,received_at=excluded.received_at,body=excluded.body,snippet=excluded.snippet", params![id,message.id,thread_id,normalized.subject,normalized.sender,normalized.received_at,normalized.body,snippet]).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute("INSERT INTO provider_messages(message_id,json) VALUES(?1,?2) ON CONFLICT(message_id) DO UPDATE SET json=excluded.json", params![id,fetched.original_json]).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute("DELETE FROM message_labels WHERE message_id=?1", [&id])
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        for label in &message.label_ids {
            tx.execute("INSERT OR IGNORE INTO labels(id) VALUES(?1)", [label])
                .map_err(|_| GMAIL_DB_ERROR.to_string())?;
            tx.execute(
                "INSERT OR IGNORE INTO message_labels VALUES(?1,?2)",
                params![id, label],
            )
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        }
        tx.execute("DELETE FROM gmail_parts WHERE message_id=?1", [&id])
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute("DELETE FROM attachments WHERE message_id=?1", [&id])
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        let mut attachment_index = 0;
        for part in &normalized.parts {
            tx.execute("INSERT INTO gmail_parts(message_id,part_id,attachment_id,mime_type,name,expected_size,is_body,content) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id,part.part_id,part.attachment_id,part.mime_type,part.name,part.size as i64,part.is_body,part.content]).map_err(|_| GMAIL_DB_ERROR.to_string())?;
            if !part.is_body {
                tx.execute("INSERT INTO attachments(message_id,part_index,name,expected_size,content) VALUES(?1,?2,?3,?4,?5)", params![id,attachment_index,part.name,part.size as i64,part.content]).map_err(|_| GMAIL_DB_ERROR.to_string())?;
                attachment_index += 1;
            }
        }
        tx.commit().map_err(|_| GMAIL_DB_ERROR.to_string())
    }

    pub fn put_part(
        &mut self,
        provider_id: &str,
        part_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.require_gmail()?;
        let id = format!("gmail:{provider_id}");
        let expected: Option<i64> = self.connection.query_row("SELECT expected_size FROM gmail_parts WHERE message_id=?1 AND part_id=?2 AND attachment_id IS NOT NULL", params![id,part_id], |r| r.get(0)).optional().map_err(|_| GMAIL_DB_ERROR.to_string())?;
        if expected != Some(bytes.len() as i64) {
            return Err("Downloaded MIME part size did not match. Retry synchronization.".into());
        }
        // Rebuild from original provider data and all completed external parts.
        // One transaction covers part bytes, normalized body, and FTS updates.
        let json: Vec<u8> = self
            .connection
            .query_row(
                "SELECT json FROM provider_messages WHERE message_id=?1",
                [&id],
                |r| r.get(0),
            )
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        let message: GmailMessage =
            serde_json::from_slice(&json).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        let mut external = ExternalParts::new();
        {
            let mut stmt = self.connection.prepare("SELECT part_id,content FROM gmail_parts WHERE message_id=?1 AND attachment_id IS NOT NULL AND content IS NOT NULL").map_err(|_| GMAIL_DB_ERROR.to_string())?;
            let rows = stmt
                .query_map([&id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                })
                .map_err(|_| GMAIL_DB_ERROR.to_string())?;
            for row in rows {
                let (part, content) = row.map_err(|_| GMAIL_DB_ERROR.to_string())?;
                external.insert(part, content);
            }
        }
        external.insert(part_id.to_string(), bytes.to_vec());
        let normalized = gmail_mime::normalize(&message, &external)?;
        let snippet: String = normalized
            .body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        let tx = self
            .connection
            .transaction()
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute(
            "UPDATE messages SET body=?1,snippet=?2 WHERE id=?3",
            params![normalized.body, snippet, id],
        )
        .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute(
            "UPDATE gmail_parts SET content=?1 WHERE message_id=?2 AND part_id=?3",
            params![bytes, id, part_id],
        )
        .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        for (index, part) in normalized.parts.iter().filter(|p| !p.is_body).enumerate() {
            tx.execute(
                "UPDATE attachments SET content=?1 WHERE message_id=?2 AND part_index=?3",
                params![part.content, id, index as i64],
            )
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        }
        tx.commit().map_err(|_| GMAIL_DB_ERROR.to_string())
    }

    pub fn delete_gmail(&mut self, provider_id: &str) -> Result<(), String> {
        self.require_gmail()?;
        let tx = self
            .connection
            .transaction()
            .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute(
            "DELETE FROM messages WHERE account_id='gmail' AND provider_id=?1",
            [provider_id],
        )
        .map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.execute("DELETE FROM threads WHERE NOT EXISTS(SELECT 1 FROM messages WHERE messages.thread_id=threads.id)", []).map_err(|_| GMAIL_DB_ERROR.to_string())?;
        tx.commit().map_err(|_| GMAIL_DB_ERROR.to_string())
    }

    fn require_gmail(&self) -> Result<(), String> {
        if self.is_gmail {
            Ok(())
        } else {
            Err("Real mail cannot be stored in the disposable fixture database.".into())
        }
    }
}

fn same_external_part(
    part: &crate::provider::MimePart,
    id: &str,
    attachment: &str,
    size: u64,
) -> bool {
    (part.part_id == id
        && part.body.attachment_id.as_deref() == Some(attachment)
        && part.body.size == size)
        || part
            .parts
            .iter()
            .any(|child| same_external_part(child, id, attachment, size))
}
