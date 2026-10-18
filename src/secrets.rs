//! Almacén de secretos: keyring del sistema como principal y clave maestra como respaldo.

use std::fmt;
use std::sync::{Arc, Mutex};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::store::Store;

// Identificadores internos estables: cambiarlos dejaría sin acceso a los secretos ya guardados.
const SERVICE: &str = "omniterm";
const SALT_KEY: &str = "kdf_salt";
const CHECK_KEY: &str = "kdf_check";
const CHECK_PLAINTEXT: &[u8] = b"omniterm-master-key-check";
const NONCE_LEN: usize = 24;

#[derive(Debug)]
pub enum SecretError {
    /// Hace falta desbloquear con la clave maestra.
    Locked,
    WrongPassword,
    Other(anyhow::Error),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Locked => write!(f, "el almacén de secretos está bloqueado"),
            SecretError::WrongPassword => write!(f, "clave maestra incorrecta"),
            SecretError::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for SecretError {}

impl From<anyhow::Error> for SecretError {
    fn from(error: anyhow::Error) -> Self {
        SecretError::Other(error)
    }
}

pub trait SecretStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretError>;
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// Cuenta bajo la que se guarda la contraseña (o passphrase) de una conexión.
pub fn account_for(connection_id: i64, kind: SecretKind) -> String {
    match kind {
        SecretKind::Password => format!("connection:{connection_id}:password"),
        SecretKind::Passphrase => format!("connection:{connection_id}:passphrase"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    Passphrase,
}

pub struct KeyringStore;

impl KeyringStore {
    /// `true` si hay un almacén del sistema utilizable (por ejemplo, Secret Service corriendo).
    pub fn probe() -> bool {
        if keyring::Entry::store_status().is_err() {
            return false;
        }
        match keyring::Entry::new(SERVICE, "probe").and_then(|entry| entry.get_password()) {
            Ok(_) | Err(keyring::Error::NoEntry) => true,
            Err(_) => false,
        }
    }

    fn entry(account: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, account).map_err(|e| SecretError::Other(e.into()))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(SecretError::Other(error.into())),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        Self::entry(account)?.set_password(secret).map_err(|e| SecretError::Other(e.into()))
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(SecretError::Other(error.into())),
        }
    }
}

/// Secretos cifrados en SQLite con una clave derivada de la clave maestra del usuario.
pub struct MasterKeyStore {
    store: Arc<Mutex<Store>>,
    key: Mutex<Option<Zeroizing<[u8; 32]>>>,
    kdf: Params,
}

impl MasterKeyStore {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        let kdf = Params::new(64 * 1024, 3, 1, Some(32)).expect("parámetros de Argon2 válidos");
        Self { store, key: Mutex::new(None), kdf }
    }

    #[cfg(test)]
    fn with_cheap_kdf(store: Arc<Mutex<Store>>) -> Self {
        let kdf = Params::new(8, 1, 1, Some(32)).expect("parámetros de Argon2 válidos");
        Self { store, key: Mutex::new(None), kdf }
    }

    pub fn is_initialized(&self) -> anyhow::Result<bool> {
        Ok(self.store.lock().unwrap().meta(SALT_KEY)?.is_some())
    }

    #[cfg(test)]
    pub fn is_unlocked(&self) -> bool {
        self.key.lock().unwrap().is_some()
    }

    /// Desbloquea; si aún no hay clave maestra, la crea con `password`.
    pub fn unlock(&self, password: &str) -> Result<(), SecretError> {
        let store = self.store.lock().unwrap();
        let (salt, new) = match store.meta(SALT_KEY)? {
            Some(salt) => (salt, false),
            None => {
                let mut salt = vec![0u8; 16];
                fill_random(&mut salt)?;
                (salt, true)
            }
        };
        let key = self.derive(password, &salt)?;

        if new {
            let check = seal(&key, "check", CHECK_PLAINTEXT)?;
            store.set_meta(SALT_KEY, &salt)?;
            store.set_meta(CHECK_KEY, &[check.0.as_slice(), check.1.as_slice()].concat())?;
        } else {
            let check = store.meta(CHECK_KEY)?.ok_or_else(|| anyhow::anyhow!("falta el valor de control"))?;
            if check.len() <= NONCE_LEN {
                return Err(anyhow::anyhow!("valor de control corrupto").into());
            }
            let (nonce, ciphertext) = check.split_at(NONCE_LEN);
            open(&key, "check", nonce, ciphertext).map_err(|_| SecretError::WrongPassword)?;
        }
        *self.key.lock().unwrap() = Some(key);
        Ok(())
    }

    /// Borra todos los secretos y la clave maestra (única salida si se olvida).
    pub fn reset(&self) -> anyhow::Result<()> {
        *self.key.lock().unwrap() = None;
        self.store.lock().unwrap().clear_secrets()
    }

    fn derive(&self, password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, SecretError> {
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.kdf.clone())
            .hash_password_into(password.as_bytes(), salt, key.as_mut())
            .map_err(|e| SecretError::Other(anyhow::anyhow!("derivando la clave: {e}")))?;
        Ok(key)
    }

    fn key(&self) -> Result<Zeroizing<[u8; 32]>, SecretError> {
        self.key.lock().unwrap().clone().ok_or(SecretError::Locked)
    }
}

impl SecretStore for MasterKeyStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        let key = self.key()?;
        let Some((nonce, ciphertext)) = self.store.lock().unwrap().get_secret(account)? else {
            return Ok(None);
        };
        let plain = open(&key, account, &nonce, &ciphertext)?;
        let text = String::from_utf8(plain.to_vec()).map_err(|e| SecretError::Other(e.into()))?;
        Ok(Some(Zeroizing::new(text)))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        let key = self.key()?;
        let (nonce, ciphertext) = seal(&key, account, secret.as_bytes())?;
        self.store.lock().unwrap().put_secret(account, &nonce, &ciphertext)?;
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.store.lock().unwrap().delete_secret(account)?;
        Ok(())
    }
}

/// Almacén elegido en el arranque: keyring del sistema si funciona, clave maestra si no.
pub enum Secrets {
    Keyring(KeyringStore),
    Master(Arc<MasterKeyStore>),
}

const FORCE_MASTER_KEY: &str = "force_master_key";

impl Secrets {
    pub fn select(store: Arc<Mutex<Store>>) -> Self {
        let forced = matches!(store.lock().unwrap().meta(FORCE_MASTER_KEY), Ok(Some(v)) if v == b"1");
        if !forced && KeyringStore::probe() {
            Secrets::Keyring(KeyringStore)
        } else {
            Secrets::Master(Arc::new(MasterKeyStore::new(store)))
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Secrets::Keyring(_) => "keyring del sistema",
            Secrets::Master(_) => "clave maestra",
        }
    }

    pub fn master(&self) -> Option<&Arc<MasterKeyStore>> {
        match self {
            Secrets::Master(master) => Some(master),
            Secrets::Keyring(_) => None,
        }
    }

    pub fn is_master_forced(store: &Store) -> bool {
        matches!(store.meta(FORCE_MASTER_KEY), Ok(Some(v)) if v == b"1")
    }

    pub fn set_master_forced(store: &Store, forced: bool) -> anyhow::Result<()> {
        store.set_meta(FORCE_MASTER_KEY, if forced { b"1" } else { b"0" })
    }

    fn inner(&self) -> &dyn SecretStore {
        match self {
            Secrets::Keyring(store) => store,
            Secrets::Master(store) => store.as_ref(),
        }
    }
}

impl SecretStore for Secrets {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        self.inner().get(account)
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.inner().set(account, secret)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.inner().delete(account)
    }
}

fn fill_random(buf: &mut [u8]) -> Result<(), SecretError> {
    getrandom::fill(buf).map_err(|e| SecretError::Other(anyhow::anyhow!("sin fuente de aleatoriedad: {e}")))
}

/// Cifra ligando el texto cifrado a `account` para que no pueda moverse a otra cuenta.
fn seal(key: &[u8; 32], account: &str, plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SecretError> {
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce)?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: account.as_bytes() })
        .map_err(|_| SecretError::Other(anyhow::anyhow!("fallo al cifrar")))?;
    Ok((nonce.to_vec(), ciphertext))
}

fn open(key: &[u8; 32], account: &str, nonce: &[u8], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, SecretError> {
    if nonce.len() != NONCE_LEN {
        return Err(SecretError::Other(anyhow::anyhow!("nonce con tamaño inválido")));
    }
    let nonce = XNonce::try_from(nonce).map_err(|_| SecretError::Other(anyhow::anyhow!("nonce con tamaño inválido")))?;
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(&nonce, Payload { msg: ciphertext, aad: account.as_bytes() })
        .map(Zeroizing::new)
        .map_err(|_| SecretError::Other(anyhow::anyhow!("no se pudo descifrar el secreto")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (Arc<Mutex<Store>>, MasterKeyStore) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        (store.clone(), MasterKeyStore::with_cheap_kdf(store))
    }

    #[test]
    fn locked_until_unlocked() {
        let (_, secrets) = fresh();
        assert!(matches!(secrets.get("a"), Err(SecretError::Locked)));
        assert!(matches!(secrets.set("a", "x"), Err(SecretError::Locked)));
    }

    #[test]
    fn stores_and_retrieves_secrets() {
        let (_, secrets) = fresh();
        assert!(!secrets.is_initialized().unwrap());
        secrets.unlock("correct horse").unwrap();
        assert!(secrets.is_initialized().unwrap());
        secrets.set("connection:1", "hunter2").unwrap();
        assert_eq!(secrets.get("connection:1").unwrap().as_deref().map(String::as_str), Some("hunter2"));
        assert!(secrets.get("connection:2").unwrap().is_none());
        secrets.delete("connection:1").unwrap();
        assert!(secrets.get("connection:1").unwrap().is_none());
    }

    #[test]
    fn wrong_password_is_rejected_and_right_one_unlocks_later() {
        let (store, first) = fresh();
        first.unlock("right").unwrap();
        first.set("connection:1", "secret").unwrap();

        let second = MasterKeyStore::with_cheap_kdf(store);
        assert!(matches!(second.unlock("wrong"), Err(SecretError::WrongPassword)));
        assert!(!second.is_unlocked());
        second.unlock("right").unwrap();
        assert_eq!(second.get("connection:1").unwrap().as_deref().map(String::as_str), Some("secret"));
    }

    #[test]
    fn ciphertext_is_bound_to_its_account() {
        let (store, secrets) = fresh();
        secrets.unlock("pw").unwrap();
        secrets.set("connection:1", "secret").unwrap();
        let (nonce, ciphertext) = store.lock().unwrap().get_secret("connection:1").unwrap().unwrap();
        store.lock().unwrap().put_secret("connection:2", &nonce, &ciphertext).unwrap();
        assert!(secrets.get("connection:2").is_err());
    }

    #[test]
    fn reset_wipes_secrets_and_master_key() {
        let (_, secrets) = fresh();
        secrets.unlock("pw").unwrap();
        secrets.set("connection:1", "secret").unwrap();
        secrets.reset().unwrap();
        assert!(!secrets.is_initialized().unwrap());
        secrets.unlock("another").unwrap();
        assert!(secrets.get("connection:1").unwrap().is_none());
    }
}
