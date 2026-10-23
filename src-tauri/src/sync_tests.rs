use super::*;
use crate::provider::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{collections::VecDeque, path::PathBuf};

// Scripted calls enforce request ordering and ensure no accidental network access.
enum Reply {
    Profile(Result<Profile, ProviderError>),
    Labels(Result<Vec<serde_json::Value>, ProviderError>),
    List(Option<String>, Result<ListPage, ProviderError>),
    Message(String, Result<FetchedMessage, ProviderError>),
    Part(String, String, Result<Vec<u8>, ProviderError>),
    History(String, Option<String>, Result<HistoryPage, ProviderError>),
}
struct Fake {
    replies: Mutex<VecDeque<Reply>>,
    pause_before_reply: Mutex<Option<SharedStore>>,
}
impl Fake {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            pause_before_reply: Mutex::new(None),
        }
    }
    fn take(&self) -> Reply {
        if let Some(db) = self.pause_before_reply.lock().unwrap().take() {
            set_paused(db.lock().unwrap().as_mut().unwrap(), true).unwrap();
        }
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected provider request")
    }
    fn exhausted(&self) {
        assert!(
            self.replies.lock().unwrap().is_empty(),
            "expected provider requests were skipped"
        );
    }
}
impl GmailSource for Fake {
    async fn profile(&self) -> Result<Profile, ProviderError> {
        match self.take() {
            Reply::Profile(result) => result,
            _ => panic!("expected another request, got profile"),
        }
    }
    async fn labels(&self) -> Result<Vec<serde_json::Value>, ProviderError> {
        if matches!(self.replies.lock().unwrap().front(), Some(Reply::Labels(_))) {
            match self.take() {
                Reply::Labels(result) => result,
                _ => unreachable!(),
            }
        } else {
            Ok(Vec::new())
        }
    }
    async fn list(&self, page: Option<String>) -> Result<ListPage, ProviderError> {
        match self.take() {
            Reply::List(expected, result) => {
                assert_eq!(page, expected);
                result
            }
            _ => panic!("expected another request, got list"),
        }
    }
    async fn message(&self, id: String) -> Result<FetchedMessage, ProviderError> {
        match self.take() {
            Reply::Message(expected, result) => {
                assert_eq!(id, expected);
                result
            }
            _ => panic!("expected another request, got message"),
        }
    }
    async fn attachment(
        &self,
        message: String,
        attachment: String,
    ) -> Result<Vec<u8>, ProviderError> {
        match self.take() {
            Reply::Part(expected_message, expected_attachment, result) => {
                assert_eq!(message, expected_message);
                assert_eq!(attachment, expected_attachment);
                result
            }
            _ => panic!("expected another request, got part"),
        }
    }
    async fn history(
        &self,
        cursor: String,
        page: Option<String>,
    ) -> Result<HistoryPage, ProviderError> {
        match self.take() {
            Reply::History(expected_cursor, expected_page, result) => {
                assert_eq!(cursor, expected_cursor);
                assert_eq!(page, expected_page);
                result
            }
            _ => panic!("expected another request, got history"),
        }
    }
}

struct Database {
    directory: PathBuf,
    shared: SharedStore,
}
impl Database {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "pithmail-sync-{}-{}-{}",
            std::process::id(),
            {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let store = Store::open_encrypted(&directory.join("mail.db"), &[7; 32]).unwrap();
        Self {
            directory,
            shared: Arc::new(Mutex::new(Some(store))),
        }
    }
    fn start(&self) {
        request_sync(self.shared.lock().unwrap().as_mut().unwrap()).unwrap();
    }
    fn with<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        f(self.shared.lock().unwrap().as_mut().unwrap())
    }
    fn reopen(&self) {
        *self.shared.lock().unwrap() = None;
        *self.shared.lock().unwrap() =
            Some(Store::open_encrypted(&self.directory.join("mail.db"), &[7; 32]).unwrap());
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        *self
            .shared
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn run(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future);
}
fn profile() -> Reply {
    Reply::Profile(Ok(Profile {
        email_address: "person@example.test".into(),
        history_id: "100".into(),
    }))
}
fn listed(ids: &[&str], next: Option<&str>) -> ListPage {
    ListPage {
        messages: ids
            .iter()
            .map(|id| MessageRef {
                id: (*id).into(),
                thread_id: (*id).into(),
            })
            .collect(),
        next_page_token: next.map(String::from),
    }
}
fn history(id: &str) -> HistoryPage {
    HistoryPage {
        history: Vec::new(),
        next_page_token: None,
        history_id: id.into(),
    }
}
fn fetched(id: &str, text: &str, external_body: bool, attachment: bool) -> FetchedMessage {
    let text_part = MimePart {
        part_id: "0".into(),
        mime_type: "text/plain".into(),
        body: PartBody {
            data: (!external_body).then(|| URL_SAFE_NO_PAD.encode(text)),
            attachment_id: external_body.then(|| "body-part".into()),
            size: text.len() as u64,
        },
        ..Default::default()
    };
    let mut parts = vec![text_part];
    if attachment {
        parts.push(MimePart {
            part_id: "1".into(),
            mime_type: "application/octet-stream".into(),
            filename: "file.bin".into(),
            body: PartBody {
                attachment_id: Some("file-part".into()),
                size: 3,
                data: None,
            },
            ..Default::default()
        });
    }
    let message = GmailMessage {
        id: id.into(),
        thread_id: id.into(),
        label_ids: vec!["INBOX".into()],
        internal_date: "1700000000000".into(),
        payload: MimePart {
            mime_type: "multipart/mixed".into(),
            headers: vec![
                Header {
                    name: "Subject".into(),
                    value: format!("Subject {id}"),
                },
                Header {
                    name: "From".into(),
                    value: "Person <person@example.test>".into(),
                },
            ],
            parts,
            ..Default::default()
        },
    };
    FetchedMessage {
        original_json: serde_json::to_vec(&message).unwrap(),
        message,
    }
}
fn not_found() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::NotFound,
        retry_after_secs: None,
        message: "Not found".into(),
    }
}
fn retry() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Retry,
        retry_after_secs: Some(30),
        message: "temporary".into(),
    }
}
async fn finish(db: &Database, fake: &Fake) {
    for _ in 0..50 {
        let status = step(&db.shared, fake).await.unwrap();
        assert!(
            status.error.is_none(),
            "unexpected durable error: {:?}",
            status.error
        );
        if status.complete {
            fake.exhausted();
            return;
        }
    }
    panic!("synchronization did not complete");
}

#[test]
fn full_sync_interleaves_bodies_and_pages_and_accounts_for_all_parts() {
    run(async {
        let db = Database::new();
        db.start();
        let fake = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&["a", "a", "b"], Some("page2")))),
            Reply::Message("a".into(), Ok(fetched("a", "안녕하세요", true, false))),
            Reply::Message("b".into(), Ok(fetched("b", "inline body", false, true))),
            Reply::Part(
                "a".into(),
                "body-part".into(),
                Ok("안녕하세요".as_bytes().to_vec()),
            ),
            Reply::List(Some("page2".into()), Ok(listed(&[], None))),
            Reply::History("100".into(), None, Ok(history("110"))),
            Reply::Part("b".into(), "file-part".into(), Ok(vec![1, 2, 3])),
        ]);
        for _ in 0..5 {
            step(&db.shared, &fake).await.unwrap();
        }
        db.with(|s| {
            assert_eq!(s.list_messages("").unwrap().len(), 2);
            assert_eq!(status(s).unwrap().pending_attachments, 1);
            assert!(!status(s).unwrap().complete);
        });
        finish(&db, &fake).await;
        db.with(|s| {
            assert!(s.message("gmail:a").unwrap().body.contains("안녕하세요"));
            assert_eq!(status(s).unwrap().discovered, 2);
            assert_eq!(checkpoint(s).unwrap().cursor.as_deref(), Some("110"));
        });
    });
}

#[test]
fn restart_resumes_committed_listing_and_pending_message_without_skipping() {
    run(async {
        let db = Database::new();
        db.start();
        let first = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&["a"], Some("page2")))),
        ]);
        step(&db.shared, &first).await.unwrap();
        step(&db.shared, &first).await.unwrap();
        step(&db.shared, &first).await.unwrap();
        first.exhausted();
        db.reopen();
        let second = Fake::new(vec![
            Reply::Message(
                "a".into(),
                Ok(fetched("a", "offline retained", false, false)),
            ),
            Reply::List(Some("page2".into()), Ok(listed(&["a"], None))),
            Reply::History("100".into(), None, Ok(history("111"))),
        ]);
        finish(&db, &second).await;
        db.reopen();
        db.with(|s| {
            assert_eq!(s.list_messages("retained").unwrap().len(), 1);
            assert!(status(s).unwrap().complete);
        });
    });
}

#[test]
fn expired_history_reconciles_unseen_messages_by_get_not_absence() {
    run(async {
        let db = Database::new();
        db.with(|s| {
            s.set_account("person@example.test").unwrap();
            s.put_gmail(&fetched("keep", "keep original", false, false))
                .unwrap();
            s.put_gmail(&fetched("gone", "deleted content", false, false))
                .unwrap();
            s.connection
                .execute(
                    "UPDATE sync_state SET phase='complete',cursor='1' WHERE id=1",
                    [],
                )
                .unwrap();
        });
        db.start();
        let fake = Fake::new(vec![
            profile(),
            Reply::History(
                "1".into(),
                None,
                Err(ProviderError {
                    kind: ProviderErrorKind::HistoryExpired,
                    retry_after_secs: None,
                    message: "expired".into(),
                }),
            ),
            profile(),
            Reply::List(None, Ok(listed(&[], None))),
            Reply::Message("gone".into(), Err(not_found())),
            Reply::Message(
                "keep".into(),
                Ok(fetched("keep", "still present", false, false)),
            ),
            Reply::History("100".into(), None, Ok(history("112"))),
        ]);
        for _ in 0..3 {
            step(&db.shared, &fake).await.unwrap();
        }
        db.with(|s| {
            assert_eq!(
                s.list_messages("").unwrap().len(),
                2,
                "an empty completed listing alone must not delete data"
            );
        });
        finish(&db, &fake).await;
        db.with(|s| {
            assert!(s.message("gmail:gone").is_err());
            assert!(s.list_messages("deleted").unwrap().is_empty());
            assert!(s
                .message("gmail:keep")
                .unwrap()
                .body
                .contains("still present"));
        });
    });
}

#[test]
fn history_checkpoint_waits_for_queued_label_updates_and_deletes_cascade() {
    run(async {
        let db = Database::new();
        db.with(|s| {
            s.set_account("person@example.test").unwrap();
            s.put_gmail(&fetched("gone", "old secret", false, true))
                .unwrap();
            s.connection
                .execute(
                    "UPDATE sync_state SET phase='complete',cursor='100' WHERE id=1",
                    [],
                )
                .unwrap();
        });
        db.start();
        let mut update = fetched("new", "updated text", false, false);
        update.message.label_ids = vec!["TRASH".into()];
        update.original_json = serde_json::to_vec(&update.message).unwrap();
        let changes = HistoryPage {
            history: vec![HistoryRecord {
                id: "101".into(),
                messages_added: vec![HistoryMessage {
                    message: MessageRef {
                        id: "new".into(),
                        thread_id: "new".into(),
                    },
                }],
                labels_added: vec![LabelChange {
                    message: MessageRef {
                        id: "new".into(),
                        thread_id: "new".into(),
                    },
                    label_ids: vec!["TRASH".into()],
                }],
                messages_deleted: vec![HistoryMessage {
                    message: MessageRef {
                        id: "gone".into(),
                        thread_id: "gone".into(),
                    },
                }],
                ..Default::default()
            }],
            history_id: "102".into(),
            next_page_token: Some("history2".into()),
        };
        let first = Fake::new(vec![
            profile(),
            Reply::History("100".into(), None, Ok(changes)),
        ]);
        step(&db.shared, &first).await.unwrap();
        step(&db.shared, &first).await.unwrap();
        step(&db.shared, &first).await.unwrap();
        db.with(|s| {
            assert_eq!(checkpoint(s).unwrap().cursor.as_deref(), Some("100"));
            assert!(s.message("gmail:gone").is_err());
            assert_eq!(
                s.connection
                    .query_row(
                        "SELECT count(*) FROM threads WHERE id='gmail:gone'",
                        [],
                        |r| r.get::<_, u32>(0)
                    )
                    .unwrap(),
                0,
                "history deletions must also remove orphaned thread metadata"
            );
            let parts: i64 = s
                .connection
                .query_row("SELECT count(*) FROM gmail_parts", [], |r| r.get(0))
                .unwrap();
            assert_eq!(parts, 0);
        });
        db.reopen();
        let second = Fake::new(vec![
            Reply::Message("new".into(), Ok(update)),
            Reply::History("100".into(), Some("history2".into()), Ok(history("105"))),
        ]);
        finish(&db, &second).await;
        db.with(|s| {
            assert_eq!(
                s.message("gmail:new").unwrap().message.labels,
                vec!["TRASH"]
            );
            assert!(s.overview().unwrap().recent.is_empty());
            assert_eq!(checkpoint(s).unwrap().cursor.as_deref(), Some("105"));
        });
    });
}

#[test]
fn retry_deadline_pause_and_failure_are_durable_and_never_complete() {
    run(async {
        let db = Database::new();
        db.start();
        let fake = Fake::new(vec![Reply::Profile(Err(retry()))]);
        let before = now();
        let progress = step(&db.shared, &fake).await.unwrap();
        assert!(progress.next_retry_at.unwrap() >= before + 30);
        assert!(!progress.complete);
        fake.exhausted();
        // No request is made before the retry deadline, including after reopen.
        db.reopen();
        let no_requests = Fake::new(vec![]);
        step(&db.shared, &no_requests).await.unwrap();
        db.with(|s| {
            set_paused(s, true).unwrap();
            s.connection
                .execute("UPDATE sync_state SET next_retry_at=NULL WHERE id=1", [])
                .unwrap();
        });
        step(&db.shared, &no_requests).await.unwrap();
        db.with(|s| {
            request_sync(s).unwrap();
            s.connection
                .execute("UPDATE sync_state SET attempts=4 WHERE id=1", [])
                .unwrap();
        });
        let failing = Fake::new(vec![Reply::Profile(Err(retry()))]);
        let state = step(&db.shared, &failing).await.unwrap();
        assert!(state.error.is_some());
        assert!(!state.complete);
        db.reopen();
        assert!(db.with(|s| status(s).unwrap().error.is_some()));
        step(&db.shared, &no_requests).await.unwrap();
        db.with(|s| {
            let resumed = request_sync(s).unwrap();
            assert!(resumed.error.is_none());
            assert!(!resumed.paused);
            assert_eq!(checkpoint(s).unwrap().attempts, 0);
            assert_eq!(
                resumed.next_retry_at, state.next_retry_at,
                "Manual retry must not bypass server-directed cooldown"
            );
        });
        step(&db.shared, &no_requests).await.unwrap();
    });
}

#[test]
fn missing_part_is_not_a_deleted_message_and_manual_retry_refetches() {
    run(async {
        let db = Database::new();
        db.start();
        let first = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&["a"], None))),
            Reply::Message("a".into(), Ok(fetched("a", "body", false, true))),
            Reply::History("100".into(), None, Ok(history("120"))),
            Reply::Part("a".into(), "file-part".into(), Err(not_found())),
        ]);
        for _ in 0..6 {
            step(&db.shared, &first).await.unwrap();
        }
        first.exhausted();
        db.with(|s| {
            assert!(status(s).unwrap().error.is_some());
            assert!(!status(s).unwrap().complete);
            assert!(s.message("gmail:a").is_ok());
            request_sync(s).unwrap();
        });
        let second = Fake::new(vec![
            profile(),
            Reply::Message("a".into(), Err(not_found())),
        ]);
        finish(&db, &second).await;
        db.with(|s| assert!(s.message("gmail:a").is_err()));
    });
}

#[test]
fn pause_during_inflight_response_survives_commit_and_resume_keeps_checkpoint() {
    run(async {
        let db = Database::new();
        db.start();
        let first = Fake::new(vec![profile()]);
        *first.pause_before_reply.lock().unwrap() = Some(db.shared.clone());
        let state = step(&db.shared, &first).await.unwrap();
        assert!(state.paused);
        assert_eq!(
            db.with(|s| checkpoint(s).unwrap().baseline),
            Some("100".into())
        );
        let no_requests = Fake::new(vec![]);
        step(&db.shared, &no_requests).await.unwrap();
        db.reopen();
        assert!(db.with(|s| status(s).unwrap().paused));
        db.with(|s| {
            set_paused(s, false).unwrap();
        });
        let second = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&[], None))),
            Reply::History("100".into(), None, Ok(history("101"))),
        ]);
        finish(&db, &second).await;
        db.with(|s| {
            let state = status(s).unwrap();
            assert_eq!(state.downloaded, 0);
            assert!(state.complete);
        });
    });
}

#[test]
fn expired_listing_page_restarts_enumeration_without_discarding_readable_mail() {
    run(async {
        let db = Database::new();
        db.start();
        let first = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&["a"], Some("expired")))),
            Reply::Message(
                "a".into(),
                Ok(fetched(
                    "a",
                    "readable after interrupted crawl",
                    false,
                    false,
                )),
            ),
            Reply::List(
                Some("expired".into()),
                Err(ProviderError {
                    kind: ProviderErrorKind::HistoryExpired,
                    retry_after_secs: None,
                    message: "page expired".into(),
                }),
            ),
        ]);
        for _ in 0..5 {
            step(&db.shared, &first).await.unwrap();
        }
        first.exhausted();
        db.with(|s| {
            assert!(s.message("gmail:a").is_ok());
            assert!(checkpoint(s).unwrap().baseline.is_none());
            assert_eq!(checkpoint(s).unwrap().generation, 2);
        });
        db.reopen();
        let second = Fake::new(vec![
            profile(),
            Reply::List(None, Ok(listed(&["a"], None))),
            Reply::Message(
                "a".into(),
                Ok(fetched("a", "current content", false, false)),
            ),
            Reply::History("100".into(), None, Ok(history("103"))),
        ]);
        finish(&db, &second).await;
        db.with(|s| {
            assert_eq!(s.list_messages("").unwrap().len(), 1);
            assert!(s
                .message("gmail:a")
                .unwrap()
                .body
                .contains("current content"));
        });
    });
}

#[test]
fn label_metadata_is_durable_and_failure_blocks_completion_without_replacing_it() {
    run(async {
        let db = Database::new();
        db.start();
        let label = serde_json::json!({"id":"Label_1","name":"프로젝트","type":"user","color":{"backgroundColor":"#ffffff","textColor":"#000000"}});
        let first = Fake::new(vec![
            profile(),
            Reply::Labels(Ok(vec![label.clone()])),
            Reply::List(None, Ok(listed(&[], None))),
            Reply::History("100".into(), None, Ok(history("110"))),
        ]);
        finish(&db, &first).await;
        db.reopen();
        db.with(|s| {
            let raw: Vec<u8> = s
                .connection
                .query_row(
                    "SELECT json FROM provider_labels WHERE label_id='Label_1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&raw).unwrap(),
                label
            );
            request_sync(s).unwrap();
            assert!(!checkpoint(s).unwrap().labels_checked);
        });
        let failing = Fake::new(vec![
            profile(),
            Reply::Labels(Err(ProviderError::permanent("labels denied"))),
        ]);
        step(&db.shared, &failing).await.unwrap();
        let state = step(&db.shared, &failing).await.unwrap();
        assert!(state.error.is_some());
        assert!(!state.complete);
        db.with(|s| {
            assert_eq!(
                s.connection
                    .query_row("SELECT count(*) FROM provider_labels", [], |r| r
                        .get::<_, u32>(0))
                    .unwrap(),
                1
            );
            assert_eq!(checkpoint(s).unwrap().cursor.as_deref(), Some("110"));
            request_sync(s).unwrap();
        });
        let replacement = Fake::new(vec![
            profile(),
            Reply::Labels(Ok(vec![
                serde_json::json!({"id":"INBOX","name":"INBOX","type":"system"}),
            ])),
            Reply::History("110".into(), None, Ok(history("120"))),
        ]);
        finish(&db, &replacement).await;
        db.with(|s| {
            assert_eq!(
                s.connection
                    .query_row("SELECT label_id FROM provider_labels", [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "INBOX"
            );
            assert!(apply_labels(s, vec![serde_json::json!({"id":"bad"})]).is_err());
            assert_eq!(
                s.connection
                    .query_row("SELECT label_id FROM provider_labels", [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "INBOX",
                "invalid complete response must not erase valid metadata"
            );
        });
    });
}

#[test]
fn later_history_change_wins_after_same_message_was_deleted() {
    run(async {
        let db = Database::new();
        db.with(|s| {
            s.set_account("person@example.test").unwrap();
            s.put_gmail(&fetched("a", "old", false, false)).unwrap();
            s.connection
                .execute(
                    "UPDATE sync_state SET phase='complete',cursor='100' WHERE id=1",
                    [],
                )
                .unwrap();
        });
        db.start();
        let message = || HistoryMessage {
            message: MessageRef {
                id: "a".into(),
                thread_id: "a".into(),
            },
        };
        let changes = HistoryPage {
            history: vec![
                HistoryRecord {
                    id: "101".into(),
                    messages_added: vec![message()],
                    messages_deleted: vec![message()],
                    ..Default::default()
                },
                HistoryRecord {
                    id: "102".into(),
                    messages_added: vec![message()],
                    ..Default::default()
                },
            ],
            next_page_token: None,
            history_id: "103".into(),
        };
        let fake = Fake::new(vec![
            profile(),
            Reply::History("100".into(), None, Ok(changes)),
            Reply::Message(
                "a".into(),
                Ok(fetched("a", "new authoritative content", false, false)),
            ),
        ]);
        finish(&db, &fake).await;
        db.with(|s| {
            assert!(s
                .message("gmail:a")
                .unwrap()
                .body
                .contains("new authoritative content"));
            assert_eq!(checkpoint(s).unwrap().cursor.as_deref(), Some("103"));
        });
    });
}

#[test]
fn account_mismatch_does_not_start_listing_or_replace_existing_mail() {
    run(async {
        let db = Database::new();
        db.with(|s| {
            s.set_account("original@example.test").unwrap();
            s.connection
                .execute(
                    "UPDATE sync_state SET phase='complete',cursor='100' WHERE id=1",
                    [],
                )
                .unwrap();
        });
        db.start();
        let fake = Fake::new(vec![profile()]);
        assert!(step(&db.shared, &fake).await.is_err());
        fake.exhausted();
        db.with(|s| {
            assert_eq!(
                s.account_email().unwrap().as_deref(),
                Some("original@example.test")
            );
            assert!(status(s).unwrap().error.is_some());
            assert!(!status(s).unwrap().complete);
        });
    });
}
