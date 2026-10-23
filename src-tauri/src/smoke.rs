//! Debug-only native WebView smoke runner; no test IPC commands or extra permissions.
use std::{sync::mpsc, time::Duration};
use tauri::{Manager, Webview};

pub fn enabled() -> bool {
    matches!(
        std::env::var("PITHMAIL_SMOKE_TEST").as_deref(),
        Ok("1" | "gmail")
    )
}

pub fn gmail_mode() -> bool {
    std::env::var("PITHMAIL_SMOKE_TEST").as_deref() == Ok("gmail")
}

/// Synthetic encrypted data only; never calls a credential store or Google.
pub fn prepare_gmail(
    mailbox: &crate::commands::Mailbox,
    path: &std::path::Path,
) -> Result<(), String> {
    use crate::{
        provider::{FetchedMessage, GmailMessage},
        storage::Store,
    };
    std::fs::create_dir_all(path.parent().ok_or("Smoke directory missing")?)
        .map_err(|_| "Smoke directory failed")?;
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| "Smoke random key failed")?;
    let mut store = Store::open_encrypted(path, &key)?;
    store.set_account("synthetic@example.test")?;
    for index in 0..105 {
        use base64::Engine;
        let body = format!("자료구조 synthetic local message {index}. Original offline text.");
        let json = serde_json::json!({
            "id":format!("m{index:03}"), "threadId":"conversation", "internalDate":"1792634400000",
            "labelIds":["INBOX", "Label_synthetic"],
            "payload": {"partId":"", "mimeType":"multipart/mixed", "headers":[
                {"name":"Subject", "value":format!("Synthetic Gmail {index:03}")},
                {"name":"From", "value":"sender@example.test"}],
                "parts":[{"partId":"0", "mimeType":"text/plain", "body":{"size":body.len(), "data":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body.as_bytes())}},
                {"partId":"1", "mimeType":"application/octet-stream", "filename":"pending.bin", "body":{"attachmentId":format!("part{index}"), "size":3}}]
            }
        });
        let message: GmailMessage =
            serde_json::from_value(json.clone()).map_err(|_| "Smoke message failed")?;
        store.put_gmail(&FetchedMessage {
            message,
            original_json: serde_json::to_vec(&json).map_err(|_| "Smoke payload failed")?,
        })?;
    }
    store
        .connection
        .execute(
            "INSERT INTO provider_labels VALUES('Label_synthetic',?1)",
            [r#"{"id":"Label_synthetic","name":"Synthetic label","type":"user"}"#.as_bytes()],
        )
        .map_err(|_| "Smoke label failed")?;
    store
        .connection
        .execute(
            "UPDATE sync_state SET phase='attachments',paused=1 WHERE id=1",
            [],
        )
        .map_err(|_| "Smoke state failed")?;
    mailbox.seed_smoke(store)
}

pub fn start(webview: Webview) {
    let app = webview.app_handle().clone();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_size(tauri::LogicalSize::new(760.0, 780.0));
    }
    std::thread::spawn(move || {
        let script = if gmail_mode() {
            include_str!("../../tests/native-gmail-smoke.js")
        } else {
            include_str!("../../tests/native-smoke.js")
        };
        if webview.eval(script).is_err() {
            eprintln!("Native smoke: could not start JavaScript driver");
            app.exit(1);
            return;
        }
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(200));
            let (send, receive) = mpsc::channel();
            let _ =
                webview.eval_with_callback("window.__pithmailSmoke || 'pending'", move |value| {
                    let _ = send.send(value);
                });
            if let Ok(result) = receive.recv_timeout(Duration::from_secs(2)) {
                if result == "\"pass\"" {
                    println!(
                        "Native smoke PASS ({})",
                        if gmail_mode() {
                            "encrypted synthetic Gmail, pagination, pending attachments, labels, offline reader/search"
                        } else {
                            "fixture reader, threading, search, theme, narrow navigation and error retries"
                        }
                    );
                    app.exit(0);
                    return;
                }
                if result.starts_with("\"FAIL:") {
                    eprintln!("Native smoke {result}");
                    app.exit(1);
                    return;
                }
            }
        }
        eprintln!("Native smoke FAIL: timeout waiting for UI/IPC");
        app.exit(1);
    });
}
