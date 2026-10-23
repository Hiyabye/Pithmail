//! Durable, one-request-at-a-time Gmail synchronization.
//!
//! Network requests never hold the SQLite mutex. A response may be applied after
//! pause, but its durable checkpoint ensures it is not lost or skipped on resume.
use crate::{
    provider::{
        FetchedMessage, GmailSource, HistoryPage, ListPage, Profile, ProviderError,
        ProviderErrorKind,
    },
    storage::Store,
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const DB_ERROR: &str = "Synchronization storage failed. Restart Pithmail and try again.";
const MAX_ATTEMPTS: u32 = 5;
type SharedStore = Arc<Mutex<Option<Store>>>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub phase: String,
    pub paused: bool,
    pub discovered: u32,
    pub downloaded: u32,
    pub pending_bodies: u32,
    pub pending_attachments: u32,
    pub error: Option<String>,
    pub complete: bool,
    pub next_retry_at: Option<i64>,
}

struct Checkpoint {
    phase: String,
    paused: bool,
    generation: i64,
    identity_checked: bool,
    labels_checked: bool,
    baseline: Option<String>,
    cursor: Option<String>,
    list_page: Option<String>,
    history_page: Option<String>,
    history_done: bool,
    attempts: u32,
    next_retry_at: Option<i64>,
    error: Option<String>,
}

fn checkpoint(store: &Store) -> Result<Checkpoint, String> {
    store.connection.query_row("SELECT phase,paused,generation,baseline,cursor,list_page,history_page,history_done,attempts,next_retry_at,last_error,identity_checked,labels_checked FROM sync_state WHERE id=1", [], |row| Ok(Checkpoint {
        phase: row.get(0)?, paused: row.get(1)?, generation: row.get(2)?, baseline: row.get(3)?, cursor: row.get(4)?, list_page: row.get(5)?, history_page: row.get(6)?, history_done: row.get(7)?, attempts: row.get(8)?, next_retry_at: row.get(9)?, error: row.get(10)?, identity_checked: row.get(11)?, labels_checked: row.get(12)?,
    })).map_err(|_| DB_ERROR.into())
}

pub fn status(store: &Store) -> Result<SyncStatus, String> {
    let state = checkpoint(store)?;
    let discovered = store
        .connection
        .query_row("SELECT count(*) FROM sync_seen", [], |r| r.get(0))
        .map_err(|_| DB_ERROR.to_string())?;
    let downloaded = store
        .connection
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .map_err(|_| DB_ERROR.to_string())?;
    let fetches: u32 = store
        .connection
        .query_row("SELECT count(*) FROM sync_work", [], |r| r.get(0))
        .map_err(|_| DB_ERROR.to_string())?;
    let bodies: u32 = store
        .connection
        .query_row(
            "SELECT count(*) FROM gmail_parts WHERE is_body=1 AND content IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|_| DB_ERROR.to_string())?;
    let pending_attachments: u32 = store
        .connection
        .query_row(
            "SELECT count(*) FROM gmail_parts WHERE is_body=0 AND content IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|_| DB_ERROR.to_string())?;
    let pending_bodies = fetches.saturating_add(bodies);
    let complete = state.phase == "complete" && pending_bodies == 0 && pending_attachments == 0;
    let phase = if state.phase != "idle" && pending_bodies > 0 {
        "bodies".into()
    } else {
        state.phase
    };
    Ok(SyncStatus {
        phase,
        paused: state.paused,
        discovered,
        downloaded,
        pending_bodies,
        pending_attachments,
        error: state.error,
        complete,
        next_retry_at: state.next_retry_at,
    })
}

/// A manual request starts incremental history, or resumes the exact unfinished step.
pub fn request_sync(store: &mut Store) -> Result<SyncStatus, String> {
    let state = checkpoint(store)?;
    if state.phase == "idle" || state.phase == "complete" {
        if let Some(cursor) = state.cursor {
            store.connection.execute("UPDATE sync_state SET phase='history',baseline=?1,history_page=NULL,history_target=NULL,history_done=0,paused=0,last_error=NULL,attempts=0,next_retry_at=NULL WHERE id=1", [cursor]).map_err(|_| DB_ERROR.to_string())?;
        } else {
            restart_listing(store)?;
        }
    } else {
        store.connection.execute("UPDATE sync_state SET paused=0,attempts=CASE WHEN last_error IS NOT NULL THEN 0 ELSE attempts END,last_error=NULL WHERE id=1", []).map_err(|_| DB_ERROR.to_string())?;
    }
    store
        .connection
        .execute(
            "UPDATE sync_state SET paused=0,identity_checked=0,labels_checked=0 WHERE id=1",
            [],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    status(store)
}

pub fn set_paused(store: &mut Store, paused: bool) -> Result<SyncStatus, String> {
    if !paused {
        return request_sync(store);
    }
    store
        .connection
        .execute("UPDATE sync_state SET paused=1 WHERE id=1", [])
        .map_err(|_| DB_ERROR.to_string())?;
    status(store)
}

fn restart_listing(store: &mut Store) -> Result<(), String> {
    let tx = store
        .connection
        .transaction()
        .map_err(|_| DB_ERROR.to_string())?;
    tx.execute_batch("DELETE FROM sync_seen; DELETE FROM sync_work;
        UPDATE sync_state SET phase='listing',generation=generation+1,identity_checked=0,labels_checked=0,baseline=NULL,cursor=NULL,list_page=NULL,listing_done=0,history_page=NULL,history_target=NULL,history_done=0,attempts=0,next_retry_at=NULL,last_error=NULL WHERE id=1;")
        .map_err(|_| DB_ERROR.to_string())?;
    tx.commit().map_err(|_| DB_ERROR.to_string())
}

enum Operation {
    Wait,
    Profile,
    Labels,
    List {
        page: Option<String>,
        generation: i64,
    },
    Message {
        id: String,
    },
    Part {
        message: String,
        part: String,
        attachment: String,
    },
    History {
        cursor: String,
        page: Option<String>,
    },
}

fn missing_part(store: &Store, body: bool) -> Result<Option<Operation>, String> {
    store.connection.query_row("SELECT m.provider_id,p.part_id,p.attachment_id FROM gmail_parts p JOIN messages m ON m.id=p.message_id WHERE p.content IS NULL AND p.is_body=?1 AND p.attachment_id IS NOT NULL ORDER BY m.received_at DESC,m.id,p.part_id LIMIT 1", [body], |row| Ok(Operation::Part { message: row.get(0)?, part: row.get(1)?, attachment: row.get(2)? })).optional().map_err(|_| DB_ERROR.into())
}

fn next_operation(store: &mut Store) -> Result<Operation, String> {
    for _ in 0..5 {
        let state = checkpoint(store)?;
        if state.paused
            || state.error.is_some()
            || state.next_retry_at.is_some_and(|due| due > now())
            || matches!(state.phase.as_str(), "idle" | "complete")
        {
            return Ok(Operation::Wait);
        }
        if !state.identity_checked || state.baseline.is_none() {
            return Ok(Operation::Profile);
        }
        if !state.labels_checked {
            return Ok(Operation::Labels);
        }
        let message = store.connection.query_row("SELECT message_id FROM sync_work ORDER BY CASE kind WHEN 'fetch' THEN 0 ELSE 1 END,message_id LIMIT 1", [], |r| r.get::<_,String>(0)).optional().map_err(|_| DB_ERROR.to_string())?;
        if let Some(id) = message {
            return Ok(Operation::Message { id });
        }
        if let Some(part) = missing_part(store, true)? {
            return Ok(part);
        }
        match state.phase.as_str() {
            "listing" => {
                return Ok(Operation::List {
                    page: state.list_page,
                    generation: state.generation,
                })
            }
            "reconciling" => {
                store.connection.execute("UPDATE sync_state SET phase='history',history_page=NULL,history_done=0,history_target=NULL WHERE id=1", []).map_err(|_| DB_ERROR.to_string())?;
            }
            "history" if !state.history_done => {
                return Ok(Operation::History {
                    cursor: state.baseline.ok_or(DB_ERROR)?,
                    page: state.history_page,
                })
            }
            "history" => {
                store
                    .connection
                    .execute("UPDATE sync_state SET phase='attachments' WHERE id=1", [])
                    .map_err(|_| DB_ERROR.to_string())?;
            }
            "attachments" => {
                if let Some(part) = missing_part(store, false)? {
                    return Ok(part);
                }
                let pending: u32 = store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM gmail_parts WHERE content IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|_| DB_ERROR.to_string())?;
                if pending != 0 {
                    return Err("Some message parts have no retrievable content. Synchronization is incomplete.".into());
                }
                store.connection.execute("UPDATE sync_state SET phase='complete',cursor=history_target,attempts=0,next_retry_at=NULL,last_error=NULL WHERE id=1 AND history_target IS NOT NULL", []).map_err(|_| DB_ERROR.to_string())?;
                if checkpoint(store)?.phase != "complete" {
                    return Err(DB_ERROR.into());
                }
                return Ok(Operation::Wait);
            }
            _ => return Err(DB_ERROR.into()),
        }
    }
    Err(DB_ERROR.into())
}

/// Apply at most one provider request. The caller owns single-flight execution.
pub async fn step<P: GmailSource>(db: &SharedStore, provider: &P) -> Result<SyncStatus, String> {
    match step_inner(db, provider).await {
        Ok(status) => Ok(status),
        Err(error) => {
            let persisted = error.clone();
            let _ = with_store(db, move |store| permanent_failure(store, &persisted)).await;
            Err(error)
        }
    }
}

async fn step_inner<P: GmailSource>(db: &SharedStore, provider: &P) -> Result<SyncStatus, String> {
    let operation = with_store(db, next_operation).await?;
    match operation {
        Operation::Wait => {}
        Operation::Profile => match provider.profile().await {
            Ok(profile) => with_store(db, move |s| apply_profile(s, profile)).await?,
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
        Operation::Labels => match provider.labels().await {
            Ok(labels) => with_store(db, move |s| apply_labels(s, labels)).await?,
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
        Operation::List { page, generation } => match provider.list(page.clone()).await {
            Ok(response) => with_store(db, move |s| apply_listing(s, response, page, generation)).await?,
            Err(error) if page.is_some() && matches!(error.kind, ProviderErrorKind::NotFound | ProviderErrorKind::HistoryExpired) => {
                with_store(db, restart_listing).await?;
            }
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
        Operation::Message { id } => match provider.message(id.clone()).await {
            Ok(message) => with_store(db, move |s| apply_message(s, &id, message)).await?,
            Err(error) if error.kind == ProviderErrorKind::NotFound => with_store(db, move |s| {
                s.delete_gmail(&id)?;
                s.connection.execute("DELETE FROM sync_work WHERE message_id=?1", [id]).map_err(|_| DB_ERROR.to_string())?;
                succeeded(s)
            }).await?,
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
        Operation::Part { message, part, attachment } => match provider.attachment(message.clone(), attachment).await {
            Ok(bytes) => with_store(db, move |s| {
                s.put_part(&message, &part, &bytes)?;
                succeeded(s)
            }).await?,
            Err(error) if error.kind == ProviderErrorKind::NotFound => with_store(db, move |s| {
                // The message may have changed or been deleted since its payload was
                // fetched. Refetch on explicit retry; never pretend a missing part
                // was downloaded, nor infer mailbox deletion from a part 404.
                s.connection.execute("INSERT INTO sync_work(message_id,kind) VALUES(?1,'fetch') ON CONFLICT(message_id) DO UPDATE SET kind='fetch'", [message]).map_err(|_| DB_ERROR.to_string())?;
                permanent_failure(s, "A message part is unavailable. Sync again to refresh the message and retry.")
            }).await?,
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
        Operation::History { cursor, page } => match provider.history(cursor, page.clone()).await {
            Ok(response) => with_store(db, move |s| apply_history(s, response, page)).await?,
            Err(error) if error.kind == ProviderErrorKind::HistoryExpired || error.kind == ProviderErrorKind::NotFound => with_store(db, restart_listing).await?,
            Err(error) => with_store(db, move |s| record_failure(s, error)).await?,
        },
    }
    with_store(db, |s| status(s)).await
}

fn apply_profile(store: &mut Store, profile: Profile) -> Result<(), String> {
    if profile.history_id.is_empty() {
        return Err("Gmail did not return a synchronization history cursor.".into());
    }
    store.set_account(&profile.email_address)?;
    store
        .connection
        .execute(
            "UPDATE sync_state SET baseline=COALESCE(baseline,?1),identity_checked=1 WHERE id=1",
            [profile.history_id],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    succeeded(store)
}

fn apply_labels(store: &mut Store, labels: Vec<serde_json::Value>) -> Result<(), String> {
    // Replace only after the complete list response has been fetched and validated.
    // Message label IDs survive missing/deleted label metadata until message refresh.
    let mut encoded = Vec::with_capacity(labels.len());
    let mut seen = std::collections::HashSet::new();
    for label in labels {
        let id = label
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
            .ok_or("Gmail returned invalid label metadata.")?
            .to_owned();
        if !seen.insert(id.clone())
            || label
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_none()
        {
            return Err("Gmail returned invalid label metadata.".into());
        }
        let json = serde_json::to_vec(&label).map_err(|_| DB_ERROR.to_string())?;
        encoded.push((id, json));
    }
    let tx = store
        .connection
        .transaction()
        .map_err(|_| DB_ERROR.to_string())?;
    tx.execute("DELETE FROM provider_labels", [])
        .map_err(|_| DB_ERROR.to_string())?;
    for (id, json) in encoded {
        tx.execute("INSERT OR IGNORE INTO labels(id) VALUES(?1)", [&id])
            .map_err(|_| DB_ERROR.to_string())?;
        tx.execute(
            "INSERT INTO provider_labels(label_id,json) VALUES(?1,?2)",
            params![id, json],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    }
    tx.execute(
        "UPDATE sync_state SET labels_checked=1,attempts=0,next_retry_at=NULL WHERE id=1",
        [],
    )
    .map_err(|_| DB_ERROR.to_string())?;
    tx.commit().map_err(|_| DB_ERROR.to_string())
}

fn apply_listing(
    store: &mut Store,
    page: ListPage,
    previous_page: Option<String>,
    generation: i64,
) -> Result<(), String> {
    if page.next_page_token.is_some() && page.next_page_token == previous_page {
        return permanent_failure(
            store,
            "Gmail repeated a listing checkpoint. Try synchronization again later.",
        );
    }
    let tx = store
        .connection
        .transaction()
        .map_err(|_| DB_ERROR.to_string())?;
    for message in page.messages {
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO sync_seen(message_id,generation) VALUES(?1,?2)",
                params![message.id, generation],
            )
            .map_err(|_| DB_ERROR.to_string())?;
        if inserted != 0 {
            tx.execute("INSERT INTO sync_work(message_id,kind) VALUES(?1,'fetch') ON CONFLICT(message_id) DO UPDATE SET kind='fetch'", [message.id]).map_err(|_| DB_ERROR.to_string())?;
        }
    }
    if let Some(token) = page.next_page_token {
        tx.execute("UPDATE sync_state SET list_page=?1 WHERE id=1", [token])
            .map_err(|_| DB_ERROR.to_string())?;
    } else {
        // A paginated listing is not a snapshot. Verify unseen IDs individually;
        // only messages.get 404 or a history deletion authorizes local removal.
        tx.execute("INSERT OR IGNORE INTO sync_work(message_id,kind) SELECT provider_id,'verify' FROM messages WHERE provider_id NOT IN (SELECT message_id FROM sync_seen WHERE generation=?1)", [generation]).map_err(|_| DB_ERROR.to_string())?;
        tx.execute(
            "UPDATE sync_state SET phase='reconciling',listing_done=1,list_page=NULL WHERE id=1",
            [],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    }
    tx.execute(
        "UPDATE sync_state SET attempts=0,next_retry_at=NULL WHERE id=1",
        [],
    )
    .map_err(|_| DB_ERROR.to_string())?;
    tx.commit().map_err(|_| DB_ERROR.to_string())
}

fn apply_message(
    store: &mut Store,
    expected_id: &str,
    message: FetchedMessage,
) -> Result<(), String> {
    if message.message.id != expected_id {
        return permanent_failure(store, "Gmail returned a mismatched message identifier.");
    }
    store.put_gmail(&message)?;
    // If interrupted here, replaying the same message is idempotent. Work is
    // removed only after normalized data, raw provider payload and parts commit.
    let tx = store
        .connection
        .transaction()
        .map_err(|_| DB_ERROR.to_string())?;
    tx.execute("DELETE FROM sync_work WHERE message_id=?1", [expected_id])
        .map_err(|_| DB_ERROR.to_string())?;
    tx.execute("INSERT OR IGNORE INTO sync_seen(message_id,generation) SELECT ?1,generation FROM sync_state WHERE id=1", [expected_id]).map_err(|_| DB_ERROR.to_string())?;
    tx.commit().map_err(|_| DB_ERROR.to_string())?;
    succeeded(store)
}

fn apply_history(
    store: &mut Store,
    page: HistoryPage,
    previous_page: Option<String>,
) -> Result<(), String> {
    if page.history_id.is_empty() {
        return permanent_failure(
            store,
            "Gmail did not return a synchronization history cursor.",
        );
    }
    if page.next_page_token.is_some() && page.next_page_token == previous_page {
        return permanent_failure(
            store,
            "Gmail repeated a history checkpoint. Try synchronization again later.",
        );
    }
    let tx = store
        .connection
        .transaction()
        .map_err(|_| DB_ERROR.to_string())?;
    let generation: i64 = tx
        .query_row("SELECT generation FROM sync_state WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(|_| DB_ERROR.to_string())?;
    for record in page.history {
        for id in record
            .messages_added
            .into_iter()
            .map(|m| m.message.id)
            .chain(record.labels_added.into_iter().map(|m| m.message.id))
            .chain(record.labels_removed.into_iter().map(|m| m.message.id))
        {
            tx.execute("INSERT INTO sync_work(message_id,kind) VALUES(?1,'fetch') ON CONFLICT(message_id) DO UPDATE SET kind='fetch'", [&id]).map_err(|_| DB_ERROR.to_string())?;
            tx.execute(
                "INSERT OR IGNORE INTO sync_seen(message_id,generation) VALUES(?1,?2)",
                params![id, generation],
            )
            .map_err(|_| DB_ERROR.to_string())?;
        }
        for removed in record.messages_deleted {
            tx.execute(
                "DELETE FROM messages WHERE provider_id=?1",
                [&removed.message.id],
            )
            .map_err(|_| DB_ERROR.to_string())?;
            tx.execute(
                "DELETE FROM sync_work WHERE message_id=?1",
                [&removed.message.id],
            )
            .map_err(|_| DB_ERROR.to_string())?;
        }
    }
    tx.execute("DELETE FROM threads WHERE NOT EXISTS (SELECT 1 FROM messages WHERE messages.thread_id=threads.id)", [])
        .map_err(|_| DB_ERROR.to_string())?;
    let done = page.next_page_token.is_none();
    tx.execute("UPDATE sync_state SET history_page=?1,history_target=?2,history_done=?3,attempts=0,next_retry_at=NULL WHERE id=1", params![page.next_page_token,page.history_id,done]).map_err(|_| DB_ERROR.to_string())?;
    tx.commit().map_err(|_| DB_ERROR.to_string())
}

fn succeeded(store: &Store) -> Result<(), String> {
    // Do not alter pause: an in-flight response may commit after the user pauses.
    store
        .connection
        .execute(
            "UPDATE sync_state SET attempts=0,next_retry_at=NULL WHERE id=1",
            [],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    Ok(())
}

fn permanent_failure(store: &Store, message: &str) -> Result<(), String> {
    store
        .connection
        .execute(
            "UPDATE sync_state SET last_error=?1,next_retry_at=NULL WHERE id=1",
            [message],
        )
        .map_err(|_| DB_ERROR.to_string())?;
    Ok(())
}

fn record_failure(store: &Store, error: ProviderError) -> Result<(), String> {
    match error.kind {
        ProviderErrorKind::Reconnect => permanent_failure(store, "Gmail authorization needs renewal. Reconnect your account to continue; downloaded mail remains available offline."),
        ProviderErrorKind::Retry => {
            let attempts = checkpoint(store)?.attempts.saturating_add(1);
            let mut random = [0u8; 1];
            getrandom::fill(&mut random).map_err(|_| "Secure retry scheduling is unavailable.".to_string())?;
            let exponential = (1u64 << attempts.min(6)) + u64::from(random[0] % 3);
            let delay = exponential.max(error.retry_after_secs.unwrap_or(0));
            let due = now().saturating_add(i64::try_from(delay).unwrap_or(i64::MAX));
            let terminal = (attempts >= MAX_ATTEMPTS).then_some("Gmail is unavailable or rate-limited. Synchronization is incomplete; Sync again to retry.");
            store.connection.execute("UPDATE sync_state SET attempts=?1,next_retry_at=?2,last_error=?3 WHERE id=1", params![attempts,due,terminal]).map_err(|_| DB_ERROR.to_string())?;
            Ok(())
        }
        _ => permanent_failure(store, "Gmail could not complete the request. Synchronization is incomplete; check the connection and Sync again."),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

async fn with_store<T, F>(db: &SharedStore, action: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> Result<T, String> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let mut locked = db.lock().map_err(|_| DB_ERROR.to_string())?;
        let store = locked
            .as_mut()
            .ok_or("Unlock the Gmail mailbox before synchronizing.")?;
        action(store)
    })
    .await
    .map_err(|_| DB_ERROR.to_string())?
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
