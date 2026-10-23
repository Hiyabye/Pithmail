use super::*;
use crate::provider::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

/// Explicit opt-in only. Never run this through a generic --include-ignored suite.
/// Emits no account identifiers, message IDs, headers, bodies, JSON, or tokens.
#[tokio::test]
#[ignore = "Requires explicit approval: uses native credentials and downloads at most one real message"]
async fn live_gmail_one_message() -> Result<(), String> {
    if std::env::var("PITHMAIL_LIVE_CHECK").as_deref() != Ok("one-message") {
        return Err("Live check requires the exact one-message opt-in.".into());
    }
    let path = PathBuf::from(
        std::env::var_os("PITHMAIL_LIVE_MAILBOX_PATH")
            .ok_or("Missing existing encrypted mailbox path.")?,
    );
    if !path.is_absolute() || !path.is_file() {
        return Err("Live check requires an existing encrypted mailbox.".into());
    }
    // No worker is started, and fixtures must not be opened by this diagnostic.
    let mailbox = Mailbox::new(PathBuf::new(), path.clone());
    let services = mailbox.ensure_live().await?;
    if !services.auth.connected().await? {
        return Err("Complete browser sign-in first.".into());
    }
    let profile = services
        .client
        .profile()
        .await
        .map_err(|error| error.message)?;
    mailbox
        .with_store(move |store| store.set_account(&profile.email_address))
        .await?;
    println!("Live check: account identity accepted; synchronization paused.");
    let page = services
        .client
        .list(None)
        .await
        .map_err(|error| error.message)?;
    println!("Live check: first ID page retrieved; no further pages requested.");
    let Some(first) = page.messages.into_iter().next() else {
        println!("Live check: mailbox is empty; no message downloaded.");
        return Ok(());
    };
    // Exactly one message invocation; no sync loop, history, labels, or attachment GETs.
    let fetched = services
        .client
        .message(first.id)
        .await
        .map_err(|error| error.message)?;
    let local_id = format!("gmail:{}", fetched.message.id);
    let verify_id = local_id.clone();
    mailbox
        .with_store(move |store| {
            let before: i64 = store
                .connection
                .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
                .map_err(|_| "Cannot count local messages.")?;
            store.put_gmail(&fetched)?;
            let detail = store.message(&local_id)?;
            if detail.message.id != local_id {
                return Err("Stored message identity differs.".into());
            }
            let original: Vec<u8> = store
                .connection
                .query_row(
                    "SELECT json FROM provider_messages WHERE message_id=?1",
                    [&local_id],
                    |row| row.get(0),
                )
                .map_err(|_| "Cannot verify preserved FULL JSON.")?;
            if original != fetched.original_json {
                return Err("Preserved FULL JSON differs.".into());
            }
            let after: i64 = store
                .connection
                .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
                .map_err(|_| "Cannot count local messages.")?;
            if after < before || after > before + 1 {
                return Err("One-message retrieval exceeded its storage bound.".into());
            }
            if !sync::status(store)?.paused {
                return Err("Synchronization must remain paused.".into());
            }
            Ok(())
        })
        .await?;
    // Close and reopen only the database, while retaining the process's mailbox lock.
    let key = tokio::task::spawn_blocking(|| database_key(&NativeSecrets::new()?, true))
        .await
        .map_err(|_| "Cannot access mailbox key.")??;
    tokio::task::spawn_blocking(move || {
        drop(
            mailbox
                .store
                .lock()
                .map_err(|_| "Mailbox unavailable.")?
                .take(),
        );
        let store = Store::open_encrypted(&path, &key)?;
        if store.message(&verify_id)?.message.id != verify_id || !sync::status(&store)?.paused {
            return Err("Encrypted reopen or persisted pause check failed.".to_string());
        }
        Ok(())
    })
    .await
    .map_err(|_| "Cannot verify encrypted reopen.")??;
    println!("Live check: one FULL message persisted, original JSON verified, encrypted reopen passed; sync stays paused. No external attachments requested.");
    Ok(())
}

struct SwitchingProvider {
    account_b: AtomicBool,
    entered: Notify,
    release: Notify,
    message_requests: std::sync::atomic::AtomicUsize,
}

fn message(id: &str) -> FetchedMessage {
    let message = GmailMessage {
        id: id.into(),
        thread_id: id.into(),
        label_ids: vec!["INBOX".into()],
        internal_date: "1700000000000".into(),
        payload: MimePart {
            mime_type: "text/plain".into(),
            headers: vec![
                Header {
                    name: "From".into(),
                    value: "sender@example.test".into(),
                },
                Header {
                    name: "Subject".into(),
                    value: "Account A original".into(),
                },
            ],
            body: PartBody {
                size: 19,
                data: Some(URL_SAFE_NO_PAD.encode(b"account A mail body")),
                ..Default::default()
            },
            ..Default::default()
        },
    };
    FetchedMessage {
        original_json: serde_json::to_vec(&message).unwrap(),
        message,
    }
}

impl GmailSource for SwitchingProvider {
    async fn profile(&self) -> Result<Profile, ProviderError> {
        Ok(Profile {
            email_address: if self.account_b.load(Ordering::Acquire) {
                "b@example.test"
            } else {
                "a@example.test"
            }
            .into(),
            history_id: "100".into(),
        })
    }
    async fn list(&self, _: Option<String>) -> Result<ListPage, ProviderError> {
        Ok(ListPage {
            messages: vec![MessageRef {
                id: "pending-a".into(),
                thread_id: "pending-a".into(),
            }],
            next_page_token: None,
        })
    }
    async fn message(&self, id: String) -> Result<FetchedMessage, ProviderError> {
        self.message_requests.fetch_add(1, Ordering::AcqRel);
        self.entered.notify_one();
        self.release.notified().await;
        if self.account_b.load(Ordering::Acquire) {
            return Err(ProviderError {
                kind: ProviderErrorKind::NotFound,
                retry_after_secs: None,
                message: "Not found in the newly authorized account".into(),
            });
        }
        Ok(message(&id))
    }
    async fn attachment(&self, _: String, _: String) -> Result<Vec<u8>, ProviderError> {
        panic!("unexpected attachment request")
    }
    async fn history(&self, _: String, _: Option<String>) -> Result<HistoryPage, ProviderError> {
        panic!("unexpected history request")
    }
}

fn mailbox() -> (Mailbox, PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "pithmail-transition-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut store = Store::open_encrypted(&directory.join("mail.db"), &[17; 32]).unwrap();
    store.set_account("a@example.test").unwrap();
    store.put_gmail(&message("existing-a")).unwrap();
    sync::request_sync(&mut store).unwrap();
    let mailbox = Mailbox::new(
        directory.join("unused-fixtures.db"),
        directory.join("mail.db"),
    );
    *mailbox.store.lock().unwrap() = Some(store);
    (mailbox, directory)
}

#[tokio::test]
async fn oauth_transition_waits_for_inflight_sync_and_requires_fresh_identity() {
    let (mailbox, directory) = mailbox();
    let provider = Arc::new(SwitchingProvider {
        account_b: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
        message_requests: std::sync::atomic::AtomicUsize::new(0),
    });
    mailbox.sync_step(provider.as_ref()).await.unwrap(); // profile A
    mailbox.sync_step(provider.as_ref()).await.unwrap(); // labels
    mailbox.sync_step(provider.as_ref()).await.unwrap(); // durable list page

    let worker_mailbox = mailbox.clone();
    let worker_provider = provider.clone();
    let worker =
        tokio::spawn(async move { worker_mailbox.sync_step(worker_provider.as_ref()).await });
    tokio::time::timeout(Duration::from_secs(2), provider.entered.notified())
        .await
        .unwrap();

    let auth_mailbox = mailbox.clone();
    let auth_provider = provider.clone();
    let mut transition = tokio::spawn(async move {
        auth_mailbox
            .with_auth_transition(async {
                auth_provider.account_b.store(true, Ordering::Release);
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if mailbox
                .with_store(|s| sync::status(s))
                .await
                .unwrap()
                .paused
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut transition)
            .await
            .is_err(),
        "OAuth credential replacement must wait for the entire in-flight step"
    );
    assert!(!provider.account_b.load(Ordering::Acquire));

    provider.release.notify_one();
    worker.await.unwrap().unwrap();
    transition.await.unwrap().unwrap();
    assert!(provider.account_b.load(Ordering::Acquire));
    assert!(
        mailbox
            .with_store(|s| sync::status(s))
            .await
            .unwrap()
            .paused
    );
    mailbox
        .with_store(|s| s.message("gmail:pending-a").map(|_| ()))
        .await
        .unwrap();

    // A previously scheduled step must observe pause after entering the gate.
    mailbox.sync_step(provider.as_ref()).await.unwrap();
    assert_eq!(provider.message_requests.load(Ordering::Acquire), 1);
    mailbox.with_store(sync::request_sync).await.unwrap();
    assert!(
        mailbox.sync_step(provider.as_ref()).await.is_err(),
        "Resume must reject account B at profile verification"
    );
    assert_eq!(provider.message_requests.load(Ordering::Acquire), 1);
    mailbox
        .with_store(|s| {
            assert_eq!(s.account_email()?.as_deref(), Some("a@example.test"));
            s.message("gmail:existing-a")?;
            s.message("gmail:pending-a")?;
            Ok(())
        })
        .await
        .unwrap();
    *mailbox.store.lock().unwrap() = None;
    drop(mailbox);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn failed_oauth_transition_leaves_sync_durably_paused() {
    let (mailbox, directory) = mailbox();
    let result: Result<(), String> = mailbox
        .with_auth_transition(async { Err("Synthetic canceled consent".into()) })
        .await;
    assert!(result.is_err());
    assert!(
        mailbox
            .with_store(|s| sync::status(s))
            .await
            .unwrap()
            .paused
    );
    *mailbox.store.lock().unwrap() = None;
    let reopened = Store::open_encrypted(&directory.join("mail.db"), &[17; 32]).unwrap();
    assert!(sync::status(&reopened).unwrap().paused);
    drop(reopened);
    drop(mailbox);
    std::fs::remove_dir_all(directory).unwrap();
}
