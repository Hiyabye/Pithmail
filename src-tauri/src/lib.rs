mod commands;
mod domain;
mod mail;
#[cfg(debug_assertions)]
mod smoke;
mod storage;

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
            app.manage(commands::Mailbox::new(path));
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
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Pithmail");
}
