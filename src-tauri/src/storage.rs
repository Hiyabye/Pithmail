use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    domain::{AttachmentInfo, MessageDetail, MessageSummary, Overview, Settings},
    mail::{self, FixtureProvider, MailProvider},
};

const DB_ERROR: &str =
    "Local mail storage failed. Retry or restart Pithmail. Keep your database and OS credentials.";
const SUMMARY_COLUMNS: &str = "m.id, m.thread_id, m.subject, m.sender, m.received_at, m.snippet";

pub struct Store {
    pub(crate) connection: Connection,
    pub(crate) is_gmail: bool,
}

impl Store {
    /// This store deliberately cannot accept a real account or a real-mail database.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|_| DB_ERROR.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| DB_ERROR.to_string())?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| DB_ERROR.to_string())?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| DB_ERROR.to_string())?;
        match version {
            0 => {
                // Refuse to initialize a pre-existing database with an unrelated schema.
                let tables: u32 = connection.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                    [], |row| row.get(0),
                ).map_err(|_| DB_ERROR.to_string())?;
                if tables != 0 {
                    return Err("Not an empty fixture database; refusing to modify it".into());
                }
                let tx = connection.transaction().map_err(|_| DB_ERROR.to_string())?;
                tx.execute_batch(include_str!("../migrations/001_fixture.sql"))
                    .map_err(|_| DB_ERROR.to_string())?;
                tx.commit().map_err(|_| DB_ERROR.to_string())?;
            }
            1 => {}
            _ => return Err("Fixture database was created by a newer application version".into()),
        }
        let kind: Option<String> = connection
            .query_row("SELECT kind FROM database_kind", [], |row| row.get(0))
            .optional()
            .map_err(|_| "This is not a Pithmail fixture database".to_string())?;
        if kind.as_deref() != Some("synthetic-fixtures") {
            return Err("This is not a Pithmail fixture database".into());
        }
        // Only alter persistent database settings after confirming fixture ownership.
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|_| DB_ERROR.to_string())?;
        Ok(Self {
            connection,
            is_gmail: false,
        })
    }

    pub fn initialize_fixtures(&mut self) -> Result<(), String> {
        if self.is_gmail {
            return Err("Fixture import cannot modify real-mail storage.".into());
        }
        let tx = self
            .connection
            .transaction()
            .map_err(|_| DB_ERROR.to_string())?;
        tx.execute("INSERT OR IGNORE INTO accounts(id,provider,address) VALUES('fixture','fixture','jihoon@example.test')", [])
            .map_err(|_| DB_ERROR.to_string())?;
        for source in FixtureProvider.messages() {
            let parsed = mail::parse(source.raw)?;
            let id = format!("fixture:{}", source.id);
            let thread_id = format!("fixture:{}", source.thread_id);
            tx.execute(
                "INSERT OR IGNORE INTO threads(id,account_id,provider_id) VALUES(?1,'fixture',?2)",
                params![thread_id, source.thread_id],
            )
            .map_err(|_| DB_ERROR.to_string())?;
            let snippet: String = parsed
                .body
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(160)
                .collect();
            tx.execute(
                "INSERT INTO messages(id,account_id,provider_id,thread_id,subject,sender,received_at,body,snippet)
                 VALUES(?1,'fixture',?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(account_id,provider_id) DO UPDATE SET thread_id=excluded.thread_id,
                 subject=excluded.subject,sender=excluded.sender,received_at=excluded.received_at,
                 body=excluded.body,snippet=excluded.snippet",
                params![id, source.id, thread_id, parsed.subject, parsed.sender, parsed.received_at, parsed.body, snippet],
            ).map_err(|_| DB_ERROR.to_string())?;
            tx.execute("INSERT INTO raw_messages(message_id,mime) VALUES(?1,?2) ON CONFLICT(message_id) DO UPDATE SET mime=excluded.mime", params![id, source.raw])
                .map_err(|_| DB_ERROR.to_string())?;
            tx.execute("DELETE FROM message_labels WHERE message_id=?1", [&id])
                .map_err(|_| DB_ERROR.to_string())?;
            for label in source.labels {
                tx.execute("INSERT OR IGNORE INTO labels(id) VALUES(?1)", [label])
                    .map_err(|_| DB_ERROR.to_string())?;
                tx.execute(
                    "INSERT INTO message_labels VALUES(?1,?2)",
                    params![id, label],
                )
                .map_err(|_| DB_ERROR.to_string())?;
            }
            tx.execute("DELETE FROM attachments WHERE message_id=?1", [&id])
                .map_err(|_| DB_ERROR.to_string())?;
            for (index, attachment) in parsed.attachments.iter().enumerate() {
                tx.execute("INSERT INTO attachments(message_id,part_index,name,content) VALUES(?1,?2,?3,?4)", params![id, index as i64, attachment.name, attachment.content])
                    .map_err(|_| DB_ERROR.to_string())?;
            }
        }
        tx.commit().map_err(|_| DB_ERROR.to_string())
    }

    pub fn list_messages(&self, query: &str) -> Result<Vec<MessageSummary>, String> {
        self.list_messages_range(query, 200, 0)
    }

    pub fn list_messages_page(
        &self,
        query: &str,
        offset: u32,
    ) -> Result<Vec<MessageSummary>, String> {
        self.list_messages_range(query, 100, offset)
    }

    fn list_messages_range(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>, String> {
        if query.chars().count() > 512 {
            return Err("Search must be at most 512 characters".into());
        }
        let tokens: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect();
        if tokens.is_empty() {
            if !query.trim().is_empty() {
                return Ok(Vec::new());
            }
            return self.summaries(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m ORDER BY m.received_at DESC,m.id LIMIT ?1 OFFSET ?2"), &[&limit,&offset]);
        }
        // Treat search as literal words, never as FTS operators or SQL.
        let expression = tokens
            .iter()
            .map(|token| format!("\"{token}\"*"))
            .collect::<Vec<_>>()
            .join(" AND ");
        self.summaries(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m JOIN messages_fts f ON f.rowid=m.rowid WHERE messages_fts MATCH ?1 ORDER BY rank,m.received_at DESC,m.id LIMIT ?2 OFFSET ?3"), &[&expression,&limit,&offset])
    }

    pub fn message(&self, id: &str) -> Result<MessageDetail, String> {
        let summaries = self.summaries(
            &format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE m.id=?1"),
            &[&id],
        )?;
        let message = summaries
            .into_iter()
            .next()
            .ok_or_else(|| "Message is no longer available locally".to_string())?;
        let body = self
            .connection
            .query_row("SELECT body FROM messages WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .map_err(|_| DB_ERROR.to_string())?;
        let attachment_sql = if self.is_gmail {
            "SELECT name,expected_size FROM attachments WHERE message_id=?1 ORDER BY part_index"
        } else {
            "SELECT name,length(content) FROM attachments WHERE message_id=?1 ORDER BY part_index"
        };
        let mut stmt = self
            .connection
            .prepare(attachment_sql)
            .map_err(|_| DB_ERROR.to_string())?;
        let attachments = stmt
            .query_map([id], |row| {
                Ok(AttachmentInfo {
                    name: row.get(0)?,
                    size: row.get::<_, i64>(1)? as u64,
                })
            })
            .map_err(|_| DB_ERROR.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR.to_string())?;
        Ok(MessageDetail {
            message,
            body,
            attachments,
        })
    }

    pub fn thread(&self, id: &str) -> Result<Vec<MessageDetail>, String> {
        self.thread_range(id, 200, 0)
    }

    pub fn thread_page(&self, id: &str, offset: u32) -> Result<Vec<MessageDetail>, String> {
        self.thread_range(id, 100, offset)
    }

    fn thread_range(
        &self,
        id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageDetail>, String> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM messages WHERE thread_id=?1 ORDER BY received_at,id LIMIT ?2 OFFSET ?3")
            .map_err(|_| DB_ERROR.to_string())?;
        let ids = stmt
            .query_map(params![id, limit, offset], |row| row.get::<_, String>(0))
            .map_err(|_| DB_ERROR.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR.to_string())?;
        ids.iter().map(|id| self.message(id)).collect()
    }

    pub fn overview(&self) -> Result<Overview, String> {
        let visible = "NOT EXISTS (SELECT 1 FROM message_labels l WHERE l.message_id=m.id AND l.label_id IN ('SPAM','TRASH'))";
        let message_count = self
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .map_err(|_| DB_ERROR.to_string())?;
        let important = self.summaries(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE {visible} AND EXISTS (SELECT 1 FROM message_labels l WHERE l.message_id=m.id AND l.label_id='IMPORTANT') ORDER BY m.received_at DESC,m.id LIMIT 10"), &[])?;
        let recent = self.summaries(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE {visible} ORDER BY m.received_at DESC,m.id LIMIT 10"), &[])?;
        Ok(Overview {
            message_count,
            important,
            recent,
        })
    }

    pub fn settings(&self) -> Result<Settings, String> {
        self.connection
            .query_row("SELECT theme FROM settings WHERE id=1", [], |row| {
                Ok(Settings { theme: row.get(0)? })
            })
            .map_err(|_| DB_ERROR.to_string())
    }

    pub fn save_settings(&self, settings: Settings) -> Result<Settings, String> {
        if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
            return Err("Theme must be system, light, or dark".into());
        }
        self.connection
            .execute("UPDATE settings SET theme=?1 WHERE id=1", [&settings.theme])
            .map_err(|_| DB_ERROR.to_string())?;
        self.settings()
    }

    fn summaries(
        &self,
        sql: &str,
        parameters: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<MessageSummary>, String> {
        let mut stmt = self
            .connection
            .prepare(sql)
            .map_err(|_| DB_ERROR.to_string())?;
        let mut messages = stmt
            .query_map(parameters, |row| {
                Ok(MessageSummary {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    subject: row.get(2)?,
                    sender: row.get(3)?,
                    received_at: row.get(4)?,
                    snippet: row.get(5)?,
                    labels: Vec::new(),
                    attachment_count: 0,
                })
            })
            .map_err(|_| DB_ERROR.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| DB_ERROR.to_string())?;
        for message in &mut messages {
            let mut labels = self
                .connection
                .prepare(if self.is_gmail {
                    "SELECT COALESCE(json_extract(p.json,'$.name'),l.label_id) FROM message_labels l LEFT JOIN provider_labels p ON p.label_id=l.label_id WHERE l.message_id=?1 ORDER BY l.label_id"
                } else {
                    "SELECT label_id FROM message_labels WHERE message_id=?1 ORDER BY label_id"
                })
                .map_err(|_| DB_ERROR.to_string())?;
            message.labels = labels
                .query_map([&message.id], |row| row.get(0))
                .map_err(|_| DB_ERROR.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| DB_ERROR.to_string())?;
            message.attachment_count = self
                .connection
                .query_row(
                    "SELECT count(*) FROM attachments WHERE message_id=?1",
                    [&message.id],
                    |row| row.get(0),
                )
                .map_err(|_| DB_ERROR.to_string())?;
        }
        Ok(messages)
    }
}

#[path = "storage_gmail.rs"]
mod gmail;
#[cfg(test)]
#[path = "storage_gmail_tests.rs"]
mod gmail_tests;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
