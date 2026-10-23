mod auth;
mod commands;
mod domain;
mod gmail;
mod gmail_mime;
mod mail;
mod network;
mod provider;
mod security;
#[cfg(debug_assertions)]
mod smoke;
mod storage;
mod sync;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let path = app
                .path()
                .app_local_data_dir()?
                .join("synthetic-fixtures-v1")
                .join("mail.sqlite3");
            #[cfg(debug_assertions)]
            let path = if smoke::enabled() {
                std::env::temp_dir()
                    .join(format!("pithmail-smoke-{}", std::process::id()))
                    .join("mail.sqlite3")
            } else {
                path
            };
            let real_path = app
                .path()
                .app_local_data_dir()?
                .join("gmail-v1")
                .join("mail.sqlite3");
            #[cfg(debug_assertions)]
            let real_path = if smoke::enabled() {
                std::env::temp_dir()
                    .join(format!("pithmail-smoke-{}", std::process::id()))
                    .join("gmail")
                    .join("mail.sqlite3")
            } else {
                real_path
            };
            let mailbox = commands::Mailbox::new(path, real_path.clone());
            #[cfg(debug_assertions)]
            if smoke::gmail_mode() {
                smoke::prepare_gmail(&mailbox, &real_path)?;
            }
            app.manage(mailbox);
            Ok(())
        })
        .on_page_load(|webview, payload| {
            #[cfg(debug_assertions)]
            if smoke::enabled()
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
            {
                smoke::start(webview.clone());
            }
            #[cfg(not(debug_assertions))]
            let _ = (webview, payload);
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_messages,
            commands::get_message,
            commands::get_thread,
            commands::get_overview,
            commands::get_settings,
            commands::save_settings,
            commands::list_messages_page,
            commands::get_thread_page,
            commands::get_connection,
            commands::open_gmail,
            commands::connect_gmail,
            commands::cancel_login,
            commands::start_sync,
            commands::pause_sync,
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Pithmail");
}
