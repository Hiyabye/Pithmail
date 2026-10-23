use super::*;

fn fixtures() -> Store {
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    store.initialize_fixtures().unwrap();
    store
}

#[test]
fn migrations_and_relational_constraints() {
    let store = fixtures();
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert!(store
        .connection
        .execute("INSERT INTO message_labels VALUES ('missing','INBOX')", [])
        .is_err());
    assert!(store
        .connection
        .execute(
            "INSERT INTO accounts VALUES ('real','gmail','real@example.test')",
            []
        )
        .is_err());
    assert!(store
        .connection
        .execute("UPDATE settings SET theme='invalid'", [])
        .is_err());
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn fixture_import_is_idempotent_and_preserves_raw_and_attachments() {
    let mut store = fixtures();
    store.initialize_fixtures().unwrap();
    assert_eq!(store.list_messages("").unwrap().len(), 6);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM raw_messages", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        6
    );
    let detail = store.message("fixture:planning-1").unwrap();
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0].name, "agenda.txt");
    assert_eq!(detail.attachments[0].size, 29);
    let content: Vec<u8> = store
        .connection
        .query_row(
            "SELECT content FROM attachments WHERE message_id='fixture:planning-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(content, b"1. Offline search\n2. Privacy\n");
    assert!(detail.body.contains("14:00"));
}

#[test]
fn mime_decodes_korean_and_converts_untrusted_html_to_text() {
    let store = fixtures();
    let korean = store.message("fixture:korean").unwrap();
    assert!(korean.message.subject.contains("자료구조"));
    assert!(korean.body.contains("자료구조"));
    let html = store.message("fixture:library").unwrap();
    assert!(html.body.contains("Library renewal"));
    assert!(!html.body.contains("<img"));
    assert!(!html.body.contains("<script"));
    assert!(!html.body.contains("<a"));
    assert!(crate::mail::parse(b"not an email").is_err());
    assert!(crate::mail::parse(b"").is_err());
}

#[test]
fn local_search_is_literal_bounded_and_supports_korean() {
    let store = fixtures();
    assert_eq!(store.list_messages("offline proposal").unwrap().len(), 1);
    assert_eq!(store.list_messages("자료구조").unwrap().len(), 1);
    assert_eq!(store.list_messages("renew").unwrap().len(), 1);
    assert!(store.list_messages("\" OR * ()").unwrap().is_empty());
    assert!(store.list_messages("!!!").unwrap().is_empty());
    assert!(store.list_messages(&"x".repeat(513)).is_err());
    assert_eq!(
        store.list_messages("unwanted").unwrap()[0].id,
        "fixture:spam"
    );
}

#[test]
fn search_index_tracks_updates_and_cascading_deletes() {
    let store = fixtures();
    store
        .connection
        .execute(
            "UPDATE messages SET body='uniquereplacement' WHERE id='fixture:library'",
            [],
        )
        .unwrap();
    assert_eq!(store.list_messages("uniquereplacement").unwrap().len(), 1);
    assert!(store.list_messages("renewed").unwrap().is_empty());
    store
        .connection
        .execute("DELETE FROM messages WHERE id='fixture:library'", [])
        .unwrap();
    assert!(store.list_messages("uniquereplacement").unwrap().is_empty());
    assert!(store.message("fixture:library").is_err());
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT count(*) FROM raw_messages WHERE message_id='fixture:library'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn overview_excludes_spam_trash_and_thread_keeps_sent_reply() {
    let store = fixtures();
    let overview = store.overview().unwrap();
    assert_eq!(overview.message_count, 6);
    assert_eq!(overview.important.len(), 2);
    assert_eq!(overview.recent.len(), 4);
    assert!(overview
        .recent
        .iter()
        .all(|m| !m.labels.iter().any(|l| l == "SPAM" || l == "TRASH")));
    let thread = store.thread("fixture:planning").unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[0].message.id, "fixture:planning-1");
    assert_eq!(thread[1].message.labels, vec!["SENT"]);
    assert!(store.thread("missing").unwrap().is_empty());
}

#[test]
fn settings_validate_and_mail_survives_reopen_without_provider() {
    let path = std::env::temp_dir().join(format!(
        "pithmail-fixture-test-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let mut store = Store::open(&path).unwrap();
        store.initialize_fixtures().unwrap();
        assert_eq!(store.settings().unwrap().theme, "system");
        store
            .save_settings(Settings {
                theme: "dark".into(),
            })
            .unwrap();
        assert!(store
            .save_settings(Settings {
                theme: "remote".into()
            })
            .is_err());
    }
    {
        let store = Store::open(&path).unwrap();
        assert_eq!(store.settings().unwrap().theme, "dark");
        assert_eq!(store.list_messages("자료구조").unwrap().len(), 1);
        assert!(store
            .message("fixture:planning-1")
            .unwrap()
            .body
            .contains("SQLite"));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unrelated_and_newer_databases_are_not_migrated() {
    let path = std::env::temp_dir().join(format!(
        "pithmail-unrelated-test-{}.sqlite",
        std::process::id()
    ));
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();
    }
    assert!(Store::open(&path)
        .unwrap_err_message()
        .contains("Not an empty"));
    {
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                .unwrap(),
            "delete",
            "Rejecting unrelated storage must not change its persistent journal mode"
        );
        connection.pragma_update(None, "user_version", 99).unwrap();
    }
    assert!(Store::open(&path).unwrap_err_message().contains("newer"));
    std::fs::remove_file(path).unwrap();
}

// Keep Store free of Debug implementations that might accidentally expose mail.
trait ErrorMessage {
    fn unwrap_err_message(self) -> String;
}
impl ErrorMessage for Result<Store, String> {
    fn unwrap_err_message(self) -> String {
        match self {
            Err(error) => error,
            Ok(_) => panic!("expected database rejection"),
        }
    }
}
