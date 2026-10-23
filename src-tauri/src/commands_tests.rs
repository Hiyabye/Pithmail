use super::*;
use crate::provider::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

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
