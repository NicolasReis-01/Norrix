use keyring_core::Entry;
use secrecy::{SecretBox, zeroize::Zeroize};
use serde::{Deserialize, Serialize, Serializer};
use std::sync::LazyLock;

pub use secrecy::{ExposeSecret, SerializableSecret};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SecretString(SecretBox<SecretValue>);

impl Default for SecretString {
    fn default() -> Self {
        Self::from(String::new())
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(SecretBox::new(Box::new(SecretValue(value))))
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl ExposeSecret<str> for SecretString {
    fn expose_secret(&self) -> &str {
        &self.0.expose_secret().0
    }
}

#[derive(Clone, Deserialize)]
#[serde(transparent)]
struct SecretValue(String);

impl Zeroize for SecretValue {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl secrecy::CloneableSecret for SecretValue {}

impl Serialize for SecretValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

impl SerializableSecret for SecretValue {}

const SERVICE: &str = "norrix";
const LEGACY_SERVICE: &str = "koharu";

static CREDENTIAL_STORE: LazyLock<Result<(), String>> = LazyLock::new(|| {
    #[cfg(target_os = "linux")]
    {
        linux_keyutils_keyring_store::Store::new()
            .map(|store| keyring_core::set_default_store(store))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    Ok(())
});

/// Load a Norrix secret by key, returning `None` when no credential exists.
pub fn get(key: &str) -> anyhow::Result<Option<SecretString>> {
    let current = entry(SERVICE, key)?;
    match current.get_password() {
        Ok(value) => Ok(Some(SecretString::from(value))),
        Err(keyring_core::Error::NoEntry) => {
            let legacy = entry(LEGACY_SERVICE, key)?;
            match legacy.get_password() {
                Ok(value) => {
                    let secret = SecretString::from(value);
                    current.set_password(secret.expose_secret())?;
                    Ok(Some(secret))
                }
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// Store a Norrix secret by key.
pub fn set(key: &str, secret: &SecretString) -> anyhow::Result<()> {
    entry(SERVICE, key)?.set_password(secret.expose_secret())?;
    Ok(())
}

/// Delete a Norrix secret by key without removing the Koharu credential.
pub fn delete(key: &str) -> anyhow::Result<()> {
    delete_entry(&entry(SERVICE, key)?)?;
    Ok(())
}

fn delete_entry(entry: &Entry) -> anyhow::Result<()> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn entry(service: &str, key: &str) -> anyhow::Result<Entry> {
    match &*CREDENTIAL_STORE {
        Ok(()) => {}
        Err(error) => anyhow::bail!("failed to initialize Linux Keyutils: {error}"),
    }
    #[cfg(target_os = "linux")]
    let entry = Entry::new(service, key)?;
    #[cfg(not(target_os = "linux"))]
    let entry = keyring::Entry::new(service, key)?.inner;
    Ok(entry)
}
