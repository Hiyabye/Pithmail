use crate::{
    auth::AuthService,
    domain::*,
    gmail::GmailClient,
    network::Network,
    provider::GmailSource,
    security::{database_key, NativeSecrets, SecretStore},
    storage::Store,
    sync::{self, SyncStatus},
};
use serde::Serialize;
use std::{
    fs::File,
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Emitter, State};
use tokio::sync::Notify;

type Database = Arc<Mutex<Option<Store>>>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    mode: String,
    email: Option<String>,
    oauth_configured: bool,
    busy: bool,
    error: Option<String>,
    sync: Option<SyncStatus>,
}

struct LiveServices {
    auth: Arc<AuthService>,
    client: GmailClient,
    // Held for the session, before any key creation. A second process must not
    // provision another key or race the durable sync state machine.
    _lock: File,
}

#[derive(Clone)]
pub struct Mailbox {
    fixture_path: PathBuf,
    real_path: PathBuf,
    config_path: Option<PathBuf>,
    store: Database,
    live: Arc<tokio::sync::Mutex<Option<Arc<LiveServices>>>>,
    transition: Arc<tokio::sync::Mutex<()>>,
    busy: Arc<AtomicBool>,
    worker_started: Arc<AtomicBool>,
    wake: Arc<Notify>,
    error: Arc<Mutex<Option<String>>>,
}

struct BusyGuard(Arc<AtomicBool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Mailbox {
    pub fn new(fixture_path: PathBuf, real_path: PathBuf) -> Self {
        Self {
            fixture_path,
            real_path,
            config_path: std::env::var_os("PITHMAIL_OAUTH_CONFIG").map(PathBuf::from),
            store: Arc::new(Mutex::new(None)),
            live: Arc::new(tokio::sync::Mutex::new(None)),
            transition: Arc::new(tokio::sync::Mutex::new(())),
            busy: Arc::new(AtomicBool::new(false)),
            worker_started: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
            error: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn seed_smoke(&self, store: Store) -> Result<(), String> {
        *self
            .store
            .lock()
            .map_err(|_| "Smoke mailbox unavailable.")? = Some(store);
        Ok(())
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
                    .fixture_path
                    .parent()
                    .ok_or("Invalid fixture storage location.")?;
                std::fs::create_dir_all(parent)
                    .map_err(|_| "Cannot create fixture storage directory.")?;
                let mut store = Store::open(&mailbox.fixture_path)?;
                store.initialize_fixtures()?;
                *guard = Some(store);
            }
            operation(guard.as_mut().ok_or("Mailbox initialization failed.")?)
        })
        .await
        .map_err(|_| "Mailbox operation failed. Restart Pithmail.")?
    }

    fn begin_action(&self) -> Result<BusyGuard, String> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Another connection operation is in progress.")?;
        Ok(BusyGuard(self.busy.clone()))
    }

    async fn ensure_live(&self) -> Result<Arc<LiveServices>, String> {
        let mut live = self.live.lock().await;
        if let Some(service) = live.as_ref() {
            return Ok(service.clone());
        }
        let path = self.real_path.clone();
        let config = self.config_path.clone();
        let (store, services) = tokio::task::spawn_blocking(move || {
            let directory = path
                .parent()
                .ok_or("Invalid encrypted storage directory.")?;
            std::fs::create_dir_all(directory)
                .map_err(|_| "Cannot create encrypted storage directory.")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Cannot secure encrypted storage directory.")?;
            }
            let lock_path = directory.join("mail.lock");
            if lock_path.is_symlink() {
                return Err("Unsafe mailbox lock path.".into());
            }
            let lock = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .map_err(|_| "Cannot open encrypted mailbox lock.")?;
            lock.try_lock().map_err(|_| {
                "Encrypted mailbox is open in another Pithmail process. Close it and retry."
            })?;
            let vault: Arc<dyn SecretStore> = Arc::new(NativeSecrets::new()?);
            let key = database_key(vault.as_ref(), path.exists())?;
            let mut store = Store::open_encrypted(&path, &key)?;
            // Unlocking is offline-only. Downloading needs a separate Sync now action.
            sync::set_paused(&mut store, true)?;
            let network = Network::new()?;
            let auth = Arc::new(AuthService::new(vault, network.clone(), config));
            let client = GmailClient::new(auth.clone(), network);
            Ok::<_, String>((
                store,
                Arc::new(LiveServices {
                    auth,
                    client,
                    _lock: lock,
                }),
            ))
        })
        .await
        .map_err(|_| "Cannot initialize encrypted mailbox.")??;
        // All current fixture operations finish before this atomic session switch.
        let shared = self.store.clone();
        tokio::task::spawn_blocking(move || {
            *shared.lock().map_err(|_| "Mailbox unavailable.")? = Some(store);
            Ok::<_, String>(())
        })
        .await
        .map_err(|_| "Cannot open encrypted mailbox.")??;
        *live = Some(services.clone());
        Ok(services)
    }

    async fn connection_status(&self) -> Result<ConnectionStatus, String> {
        let configured = self.config_path.as_ref().is_some_and(|path| path.is_file());
        let busy = self.busy.load(Ordering::Acquire);
        let error = self
            .error
            .lock()
            .map_err(|_| "Connection state unavailable.")?
            .clone();
        self.with_store(move |store| {
            Ok(ConnectionStatus {
                mode: if store.is_gmail { "gmail" } else { "fixture" }.into(),
                email: if store.is_gmail {
                    store.account_email()?
                } else {
                    None
                },
                oauth_configured: configured,
                busy,
                error,
                sync: if store.is_gmail {
                    Some(sync::status(store)?)
                } else {
                    None
                },
            })
        })
        .await
    }

    fn record_error(&self, error: Option<String>) {
        if let Ok(mut current) = self.error.lock() {
            *current = error;
        }
    }

    async fn with_auth_transition<T>(
        &self,
        operation: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.with_store(|store| sync::set_paused(store, true))
            .await?;
        self.wake.notify_one();
        // An in-flight request may safely commit under its original credentials.
        // Do not replace those credentials until that entire step has finished.
        let _transition = self.transition.lock().await;
        operation.await
    }

    async fn sync_step<P: GmailSource>(&self, provider: &P) -> Result<SyncStatus, String> {
        let _transition = self.transition.lock().await;
        // step reads the durable pause state after acquiring the transition gate.
        sync::step(&self.store, provider).await
    }

    fn start_worker(&self, services: Arc<LiveServices>, app: tauri::AppHandle) {
        self.wake.notify_one();
        if self.worker_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let mailbox = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let status = mailbox.with_store(|store| sync::status(store)).await;
                let status = match status {
                    Ok(status) => status,
                    Err(error) => {
                        mailbox.record_error(Some(error));
                        mailbox.wake.notified().await;
                        continue;
                    }
                };
                if status.paused
                    || status.complete
                    || status.error.is_some()
                    || status.phase == "idle"
                {
                    // Notify stores a permit, so a concurrent user resume cannot be lost.
                    mailbox.wake.notified().await;
                    continue;
                }
                if let Some(deadline) = status.next_retry_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    if deadline > now {
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs((deadline-now) as u64)) => {},
                            _ = mailbox.wake.notified() => {},
                        }
                        continue;
                    }
                }
                match mailbox.sync_step(&services.client).await {
                    Ok(status) => {
                        let _ = app.emit("sync-progress", status);
                    }
                    Err(error) => mailbox.record_error(Some(error)),
                }
            }
        });
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
pub async fn list_messages_page(
    mailbox: State<'_, Mailbox>,
    query: String,
    offset: u32,
) -> Result<Vec<MessageSummary>, String> {
    mailbox
        .with_store(move |s| s.list_messages_page(&query, offset))
        .await
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
pub async fn get_thread_page(
    mailbox: State<'_, Mailbox>,
    id: String,
    offset: u32,
) -> Result<Vec<MessageDetail>, String> {
    mailbox
        .with_store(move |s| s.thread_page(&id, offset))
        .await
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
#[tauri::command]
pub async fn get_connection(mailbox: State<'_, Mailbox>) -> Result<ConnectionStatus, String> {
    mailbox.connection_status().await
}
#[tauri::command]
pub async fn open_gmail(mailbox: State<'_, Mailbox>) -> Result<ConnectionStatus, String> {
    let guard = mailbox.begin_action()?;
    let result = mailbox.ensure_live().await;
    drop(guard);
    if let Err(error) = result {
        mailbox.record_error(Some(error.clone()));
        return Err(error);
    }
    mailbox.record_error(None);
    mailbox.connection_status().await
}
#[tauri::command]
pub async fn connect_gmail(mailbox: State<'_, Mailbox>) -> Result<ConnectionStatus, String> {
    let guard = mailbox.begin_action()?;
    let result = async {
        let live = mailbox.ensure_live().await?;
        mailbox.with_auth_transition(live.auth.connect()).await
    }
    .await;
    drop(guard);
    if let Err(error) = result {
        mailbox.record_error(Some(error.clone()));
        return Err(error);
    }
    mailbox.record_error(None);
    mailbox.connection_status().await
}
#[tauri::command]
pub async fn cancel_login(mailbox: State<'_, Mailbox>) -> Result<(), String> {
    if let Some(live) = mailbox.live.lock().await.as_ref().cloned() {
        live.auth.cancel().await;
    }
    Ok(())
}
#[tauri::command]
pub async fn start_sync(
    mailbox: State<'_, Mailbox>,
    app: tauri::AppHandle,
) -> Result<ConnectionStatus, String> {
    let _guard = mailbox.begin_action()?;
    let services = mailbox.ensure_live().await?;
    if !services.auth.connected().await? {
        return Err(
            "Connect Gmail before starting synchronization. Offline mail remains available.".into(),
        );
    }
    mailbox.with_store(sync::request_sync).await?;
    mailbox.record_error(None);
    mailbox.start_worker(services, app);
    drop(_guard);
    mailbox.connection_status().await
}
#[tauri::command]
pub async fn pause_sync(mailbox: State<'_, Mailbox>) -> Result<ConnectionStatus, String> {
    mailbox
        .with_store(|store| sync::set_paused(store, true))
        .await?;
    mailbox.wake.notify_one();
    mailbox.connection_status().await
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
