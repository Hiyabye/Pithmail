//! OS-owned secrets. There is deliberately no filesystem or plaintext fallback.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

const SERVICE: &str = "app.pithmail.desktop";
const DATABASE_KEY: &str = "mail.database-key.v1";
static KEY_PROVISIONING: Mutex<()> = Mutex::new(());
const VAULT_ERROR: &str = "Secure credential storage is unavailable or locked. Unlock macOS Keychain or Linux Secret Service and retry.";

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn set(&self, name: &str, value: &[u8]) -> Result<(), String>;
}

/// Explicit platform stores avoid a global, silently selected backend.
/// Serial access also accommodates native stores with thread-sensitive credentials.
pub struct NativeSecrets {
    store: Arc<keyring_core::CredentialStore>,
    access: Mutex<()>,
}

impl NativeSecrets {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        let store: Arc<keyring_core::CredentialStore> =
            apple_native_keyring_store::keychain::Store::new()
                .map_err(|_| VAULT_ERROR.to_string())?;
        #[cfg(target_os = "linux")]
        let store: Arc<keyring_core::CredentialStore> =
            dbus_secret_service_keyring_store::Store::new().map_err(|_| VAULT_ERROR.to_string())?;
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err("Secure credential storage is not implemented on this platform.".into());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Ok(Self {
            store,
            access: Mutex::new(()),
        })
    }
}

impl SecretStore for NativeSecrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let _guard = self.access.lock().map_err(|_| VAULT_ERROR.to_string())?;
        let entry = self
            .store
            .build(SERVICE, name, None)
            .map_err(|_| VAULT_ERROR.to_string())?;
        match entry.get_password() {
            Ok(value) => {
                let value = Zeroizing::new(value);
                STANDARD.decode(value.as_bytes()).map(|bytes| Some(Zeroizing::new(bytes)))
                    .map_err(|_| "Stored credentials are damaged. Do not replace the mailbox key; recover your credential store.".into())
            }
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(VAULT_ERROR.into()),
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), String> {
        let _guard = self.access.lock().map_err(|_| VAULT_ERROR.to_string())?;
        let entry = self
            .store
            .build(SERVICE, name, None)
            .map_err(|_| VAULT_ERROR.to_string())?;
        // KWallet's Secret Service implementation requires UTF-8 secrets.
        let encoded = Zeroizing::new(STANDARD.encode(value));
        entry.set_password(&encoded).map_err(|_| VAULT_ERROR.into())
    }
}

pub fn database_key(
    vault: &dyn SecretStore,
    database_exists: bool,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let _provisioning = KEY_PROVISIONING
        .lock()
        .map_err(|_| "Mailbox key initialization failed.".to_string())?;
    if let Some(bytes) = vault.get(DATABASE_KEY)? {
        let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| "The mailbox key is damaged. Restore the original credential store; no replacement key was created.".to_string())?;
        return Ok(Zeroizing::new(key));
    }
    if database_exists {
        return Err("The encrypted mailbox exists but its key is missing. Restore the original credential store. No replacement key was created.".into());
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| "Secure randomness is unavailable.".to_string())?;
    vault.set(DATABASE_KEY, key.as_slice())?;
    let saved = vault
        .get(DATABASE_KEY)?
        .ok_or("The mailbox key could not be verified in secure storage.")?;
    if saved.as_slice() != key.as_slice() {
        return Err("The mailbox key could not be verified in secure storage.".into());
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);
    impl SecretStore for MemorySecrets {
        fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .map(Zeroizing::new))
        }
        fn set(&self, name: &str, value: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().insert(name.into(), value.into());
            Ok(())
        }
    }
    #[test]
    #[ignore = "Requires explicit approval: creates, reads and deletes one synthetic OS credential"]
    fn native_credential_roundtrip() {
        let vault = NativeSecrets::new().unwrap();
        let name = format!(
            "diagnostic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(vault.get(&name).unwrap().is_none());
        let expected = b"Pithmail synthetic credential diagnostic";
        vault.set(&name, expected).unwrap();
        let read = vault.get(&name);
        // Cleanup happens before any readback assertion, including failed readback.
        let entry = vault.store.build(SERVICE, &name, None).unwrap();
        let deleted = entry.delete_credential();
        assert_eq!(read.unwrap().unwrap().as_slice(), expected);
        deleted.unwrap();
        assert!(vault.get(&name).unwrap().is_none());
    }

    #[test]
    fn key_creation_and_reopen_preserve_key() {
        let vault = MemorySecrets::default();
        let key = database_key(&vault, false).unwrap();
        assert!(key.iter().any(|byte| *byte != 0));
        assert_eq!(*key, *database_key(&vault, true).unwrap());
    }
    #[test]
    fn existing_database_never_gets_a_replacement_key() {
        let vault = MemorySecrets::default();
        assert!(database_key(&vault, true).is_err());
        assert!(vault.0.lock().unwrap().is_empty());
        vault.set(DATABASE_KEY, b"broken").unwrap();
        assert!(database_key(&vault, false).is_err());
        assert_eq!(
            vault.get(DATABASE_KEY).unwrap().unwrap().as_slice(),
            b"broken"
        );
    }
    #[test]
    fn locked_vault_never_falls_back() {
        struct Locked;
        impl SecretStore for Locked {
            fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
                Err("locked".into())
            }
            fn set(&self, _: &str, _: &[u8]) -> Result<(), String> {
                panic!("must not write after failed read")
            }
        }
        assert!(database_key(&Locked, false).is_err());
    }
}
