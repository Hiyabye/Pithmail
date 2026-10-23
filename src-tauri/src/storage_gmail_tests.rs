use super::*;
use crate::{gmail_mime, provider::*};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

fn db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "pithmail-gmail-{name}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn full(id: &str, text: &[u8]) -> FetchedMessage {
    let message = GmailMessage {
        id: id.into(),
        thread_id: "thread1".into(),
        label_ids: vec!["INBOX".into()],
        internal_date: "1700000000000".into(),
        payload: MimePart {
            part_id: "0".into(),
            mime_type: "text/plain".into(),
            headers: vec![
                Header {
                    name: "Subject".into(),
                    value: "=?UTF-8?B?7J6Q66OM6rWs7KGw?=".into(),
                },
                Header {
                    name: "From".into(),
                    value: "Sender <sender@example.test>".into(),
                },
                Header {
                    name: "Content-Type".into(),
                    value: "text/plain; charset=utf-8".into(),
                },
            ],
            body: PartBody {
                data: Some(URL_SAFE_NO_PAD.encode(text)),
                size: text.len() as u64,
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let original_json = serde_json::to_vec(&message).unwrap();
    FetchedMessage {
        message,
        original_json,
    }
}

fn serialize(fetched: &mut FetchedMessage) {
    fetched.original_json = serde_json::to_vec(&fetched.message).unwrap();
}

#[test]
fn encrypted_storage_hides_mail_and_reopens_with_fts_only_with_correct_key() {
    let path = db_path("encrypted");
    let key = [37; 32];
    {
        let mut store = Store::open_encrypted(&path, &key).unwrap();
        store.set_account("owner@gmail.com").unwrap();
        store
            .put_gmail(&full("a", b"privateencryptionneedle offline search"))
            .unwrap();
        assert_eq!(
            store
                .list_messages("privateencryptionneedle")
                .unwrap()
                .len(),
            1
        );
        assert!(store.initialize_fixtures().is_err());
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        let wal = std::fs::read(format!("{}-wal", path.display())).unwrap();
        assert!(!wal
            .windows(b"privateencryptionneedle".len())
            .any(|w| w == b"privateencryptionneedle"));
    }
    let plain = Connection::open(&path).unwrap();
    assert!(plain
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r
            .get::<_, u32>(0))
        .is_err());
    drop(plain);
    assert!(Store::open_encrypted(&path, &[38; 32]).is_err());
    {
        let store = Store::open_encrypted(&path, &key).unwrap();
        assert_eq!(
            store.account_email().unwrap().as_deref(),
            Some("owner@gmail.com")
        );
        assert_eq!(
            store
                .list_messages("privateencryptionneedle")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.message("gmail:a").unwrap().message.subject,
            "자료구조"
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn encrypted_store_keeps_one_account_and_refuses_fixture_database() {
    let path = db_path("account");
    {
        let mut store = Store::open_encrypted(&path, &[23; 32]).unwrap();
        store.set_account("Owner@gmail.com").unwrap();
        store.set_account("owner@gmail.com").unwrap();
        assert!(store.set_account("other@gmail.com").is_err());
        assert!(store.set_account("bad\r\naddress").is_err());
    }
    std::fs::remove_file(path).unwrap();
    let fixture_path = db_path("fixture");
    drop(Store::open(&fixture_path).unwrap());
    assert!(Store::open_encrypted(&fixture_path, &[23; 32]).is_err());
    std::fs::remove_file(fixture_path).unwrap();
}

#[test]
fn external_body_and_attachment_are_durable_accounted_and_not_redownloaded_on_labels() {
    let path = db_path("parts");
    let mut fetched = full("parts", b"");
    let headers = fetched.message.payload.headers.clone();
    fetched.message.payload = MimePart {
        part_id: "root".into(),
        mime_type: "multipart/mixed".into(),
        headers,
        parts: vec![
            MimePart {
                part_id: "0".into(),
                mime_type: "text/plain".into(),
                headers: vec![Header {
                    name: "Content-Type".into(),
                    value: "text/plain; charset=utf-8".into(),
                }],
                body: PartBody {
                    attachment_id: Some("body-id".into()),
                    size: "회의 자료구조".len() as u64,
                    data: None,
                },
                ..Default::default()
            },
            MimePart {
                part_id: "1".into(),
                mime_type: "text/plain".into(),
                filename: "notes.txt".into(),
                body: PartBody {
                    attachment_id: Some("attachment-id".into()),
                    size: 4,
                    data: None,
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    serialize(&mut fetched);
    {
        let mut store = Store::open_encrypted(&path, &[49; 32]).unwrap();
        store.set_account("owner@gmail.com").unwrap();
        store.put_gmail(&fetched).unwrap();
        assert!(store.message("gmail:parts").unwrap().body.is_empty());
        assert_eq!(store.message("gmail:parts").unwrap().attachments[0].size, 4);
        assert!(store.put_part("parts", "1", b"bad").is_err());
        store
            .put_part("parts", "0", "회의 자료구조".as_bytes())
            .unwrap();
        assert_eq!(store.list_messages("회의").unwrap().len(), 1);
    }
    {
        let mut store = Store::open_encrypted(&path, &[49; 32]).unwrap();
        store.put_part("parts", "1", b"todo").unwrap();
        assert!(!store.message("gmail:parts").unwrap().body.contains("todo"));
        fetched.message.label_ids = vec!["TRASH".into()];
        serialize(&mut fetched);
        store.put_gmail(&fetched).unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM gmail_parts WHERE content IS NULL",
                    [],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(store.list_messages("").unwrap().len(), 1);
        assert!(store.overview().unwrap().recent.is_empty());
        assert_eq!(
            store
                .connection
                .query_row("SELECT json FROM provider_messages", [], |r| r
                    .get::<_, Vec<u8>>(0))
                .unwrap(),
            fetched.original_json
        );
        store.delete_gmail("parts").unwrap();
        assert!(store.list_messages("회의").unwrap().is_empty());
        for table in [
            "gmail_parts",
            "attachments",
            "provider_messages",
            "message_labels",
            "threads",
        ] {
            assert_eq!(
                store
                    .connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u32>(0))
                    .unwrap(),
                0
            );
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn gmail_mime_preserves_decoded_text_prefers_plain_and_blocks_html_markup() {
    let mut fetched = full("mime", b"already=3Ddecoded");
    fetched.message.payload.headers.push(Header {
        name: "Content-Transfer-Encoding".into(),
        value: "quoted-printable".into(),
    });
    assert_eq!(
        gmail_mime::normalize(&fetched.message, &ExternalParts::new())
            .unwrap()
            .body,
        "already=3Ddecoded"
    );
    let plain = fetched.message.payload.clone();
    let html_text =
        b"<p>HTML fallback</p><img src='https://tracker.invalid/pixel'><script>evil()</script>";
    let html = MimePart {
        part_id: "html".into(),
        mime_type: "text/html".into(),
        body: PartBody {
            data: Some(URL_SAFE_NO_PAD.encode(html_text)),
            size: html_text.len() as u64,
            ..Default::default()
        },
        ..Default::default()
    };
    fetched.message.payload = MimePart {
        part_id: "root".into(),
        mime_type: "multipart/alternative".into(),
        parts: vec![html.clone(), plain],
        ..Default::default()
    };
    assert_eq!(
        gmail_mime::normalize(&fetched.message, &ExternalParts::new())
            .unwrap()
            .body,
        "already=3Ddecoded"
    );
    fetched.message.payload.parts = vec![html];
    let text = gmail_mime::normalize(&fetched.message, &ExternalParts::new())
        .unwrap()
        .body;
    assert!(text.contains("HTML fallback"));
    assert!(!text.contains("<img"));
    assert!(!text.contains("<script"));
    fetched
        .message
        .payload
        .parts
        .push(fetched.message.payload.parts[0].clone());
    assert!(gmail_mime::normalize(&fetched.message, &ExternalParts::new()).is_err());
    let mut invalid = full("bad", b"safe");
    invalid.message.payload.body.data = Some("not!!base64".into());
    assert!(gmail_mime::normalize(&invalid.message, &ExternalParts::new()).is_err());
}

#[test]
fn gmail_charset_and_unnamed_external_inventory_are_normalized() {
    let mut fetched = full("charset", b"caf\xe9");
    fetched
        .message
        .payload
        .headers
        .iter_mut()
        .find(|h| h.name.eq_ignore_ascii_case("Content-Type"))
        .unwrap()
        .value = "text/plain; charset=iso-8859-1".into();
    assert_eq!(
        gmail_mime::normalize(&fetched.message, &ExternalParts::new())
            .unwrap()
            .body,
        "café"
    );
    fetched.message.payload.mime_type = "image/png".into();
    fetched.message.payload.body = PartBody {
        attachment_id: Some("unnamed-external".into()),
        size: 4,
        data: None,
    };
    let normalized = gmail_mime::normalize(&fetched.message, &ExternalParts::new()).unwrap();
    assert!(normalized.body.is_empty());
    assert_eq!(normalized.parts.len(), 1);
    assert!(!normalized.parts[0].is_body);
    assert_eq!(normalized.parts[0].name, "unnamed attachment");
    assert!(normalized.parts[0].content.is_none());
}

#[test]
fn paginated_encrypted_mail_has_no_silent_200_message_limit() {
    let path = db_path("pages");
    {
        let mut store = Store::open_encrypted(&path, &[91; 32]).unwrap();
        store.set_account("owner@gmail.com").unwrap();
        for i in 0..205 {
            store
                .put_gmail(&full(&format!("m{i:03}"), b"paginatedneedle"))
                .unwrap();
        }
        assert_eq!(store.list_messages_page("", 0).unwrap().len(), 100);
        assert_eq!(
            store
                .list_messages_page("paginatedneedle", 100)
                .unwrap()
                .len(),
            100
        );
        assert_eq!(store.list_messages_page("", 200).unwrap().len(), 5);
        assert_eq!(store.thread_page("gmail:thread1", 200).unwrap().len(), 5);
        assert!(store.list_messages_page("", 300).unwrap().is_empty());
    }
    std::fs::remove_file(path).unwrap();
}
