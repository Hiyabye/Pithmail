use crate::{domain::*, storage::Store};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::State;

// A single connection serializes short local reads/writes. All access, including
// first-open migrations and fixture import, happens on Tokio's blocking pool.
#[derive(Clone)]
pub struct Mailbox {
    path: PathBuf,
    store: Arc<Mutex<Option<Store>>>,
}

impl Mailbox {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            store: Arc::new(Mutex::new(None)),
        }
    }

    async fn with_store<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut Store) -> Result<T, String> + Send + 'static,
    {
        let mailbox = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = mailbox
                .store
                .lock()
                .map_err(|_| "Mailbox unavailable. Restart Pithmail.")?;
            if guard.is_none() {
                let parent = mailbox
                    .path
                    .parent()
                    .ok_or("Invalid fixture storage location.")?;
                std::fs::create_dir_all(parent)
                    .map_err(|_| "Cannot create fixture storage directory.")?;
                let mut store = Store::open(&mailbox.path)?;
                store.initialize_fixtures()?;
                *guard = Some(store);
            }
            operation(guard.as_mut().ok_or("Mailbox initialization failed.")?)
        })
        .await
        .map_err(|_| "Mailbox operation failed. Restart Pithmail.")?
    }
}

#[tauri::command]
pub async fn list_messages(
    mailbox: State<'_, Mailbox>,
    query: String,
) -> Result<Vec<MessageSummary>, String> {
    mailbox.with_store(move |s| s.list_messages(&query)).await
}

#[tauri::command]
pub async fn get_message(mailbox: State<'_, Mailbox>, id: String) -> Result<MessageDetail, String> {
    mailbox.with_store(move |s| s.message(&id)).await
}

#[tauri::command]
pub async fn get_thread(
    mailbox: State<'_, Mailbox>,
    id: String,
) -> Result<Vec<MessageDetail>, String> {
    mailbox.with_store(move |s| s.thread(&id)).await
}

#[tauri::command]
pub async fn get_overview(mailbox: State<'_, Mailbox>) -> Result<Overview, String> {
    mailbox.with_store(|s| s.overview()).await
}

#[tauri::command]
pub async fn get_settings(mailbox: State<'_, Mailbox>) -> Result<Settings, String> {
    mailbox.with_store(|s| s.settings()).await
}

#[tauri::command]
pub async fn save_settings(
    mailbox: State<'_, Mailbox>,
    settings: Settings,
) -> Result<Settings, String> {
    mailbox.with_store(move |s| s.save_settings(settings)).await
}
