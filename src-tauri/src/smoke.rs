//! Debug-only native WebView smoke runner; no test IPC commands or extra permissions.
use std::{sync::mpsc, time::Duration};
use tauri::{Manager, Webview};

pub fn enabled() -> bool {
    std::env::var("PITHMAIL_SMOKE_TEST").as_deref() == Ok("1")
}

pub fn start(webview: Webview) {
    let app = webview.app_handle().clone();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_size(tauri::LogicalSize::new(760.0, 780.0));
    }
    std::thread::spawn(move || {
        if webview
            .eval(include_str!("../../tests/native-smoke.js"))
            .is_err()
        {
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
                    println!("Native smoke PASS: ready, Today, Inbox, MIME reader, attachments, thread, Korean search, theme persistence, HTML isolation, narrow navigation, reader/theme error retries");
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
