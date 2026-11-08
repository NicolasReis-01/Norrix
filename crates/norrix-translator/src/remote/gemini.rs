// Ported from:
// https://github.com/koharu-rs/koharu/blob/f4ce03999ed1ae2faaec938dd52c2f41a87d03d9/crates/koharu-llm/src/providers/gemini.rs
// Model discovery:
// https://ai.google.dev/api/models

use std::{
    future::Future,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;
use norrix_secrets::{ExposeSecret, SecretString};
use reqwest::{Client, RequestBuilder, StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;
use uuid::Uuid;

use crate::{
    GenerationConfig as TranslationGeneration, Model, Provider, Result, TranslationRequest,
    backend::encode_image, prompt,
};

const ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const ADDITIONAL_KEYS_SECRET: &str = "gemini-rotation-keys";
const PRIMARY_KEY_ID_SECRET: &str = "gemini-primary-key-id";
const MAX_API_KEYS: usize = 16;
const KEY_TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(default)]
pub struct GeminiConfig {}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct GeminiApiKeyInfo {
    pub id: String,
    pub is_primary: bool,
    pub status: GeminiApiKeyStatus,
    pub retry_after_seconds: Option<u32>,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GeminiApiKeyStatus {
    Available,
    CoolingDown,
    Rejected,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GeminiApiKeyTestResult {
    Verified,
    Rejected,
    Unavailable,
}

#[derive(Clone)]
struct StoredKey {
    id: String,
    secret: SecretString,
    is_primary: bool,
}

#[derive(Deserialize)]
struct StoredAdditionalKey {
    id: String,
    key: SecretString,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredAdditionalKeys {
    Legacy(Vec<SecretString>),
    Managed(Vec<StoredAdditionalKey>),
}

#[derive(Serialize)]
struct StoredAdditionalKeyRef<'a> {
    id: &'a str,
    key: &'a str,
}

#[derive(Clone)]
struct ApiKey {
    secret: SecretString,
}

#[derive(Default)]
struct KeyRotation {
    next_index: usize,
    cooldowns: Vec<(SecretString, Instant)>,
    invalid: Vec<SecretString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetryAction {
    Retry,
    RejectKey,
    Stop,
}

struct AttemptFailure {
    action: RetryAction,
    reason: String,
    cooldown: Option<Duration>,
}

static KEY_ROTATION: OnceLock<Mutex<KeyRotation>> = OnceLock::new();
static KEY_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn rotation_state() -> &'static Mutex<KeyRotation> {
    KEY_ROTATION.get_or_init(|| Mutex::new(KeyRotation::default()))
}

fn key_store_lock() -> &'static Mutex<()> {
    KEY_STORE_LOCK.get_or_init(|| Mutex::new(()))
}

fn normalize_keys(keys: Vec<SecretString>) -> anyhow::Result<Vec<SecretString>> {
    let mut unique = Vec::new();
    for key in keys {
        let value = key.expose_secret();
        if value.trim().is_empty()
            || unique
                .iter()
                .any(|existing: &SecretString| existing.expose_secret() == value)
        {
            continue;
        }
        unique.push(key);
    }
    if unique.len() > MAX_API_KEYS {
        anyhow::bail!("at most {MAX_API_KEYS} Gemini API keys can be configured");
    }
    Ok(unique)
}

pub(super) fn stored_api_keys() -> anyhow::Result<Vec<SecretString>> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    Ok(load_key_records()?
        .into_iter()
        .map(|record| record.secret)
        .collect())
}

pub(super) fn set_api_keys(keys: &[SecretString]) -> anyhow::Result<()> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let previous = load_key_records()?;
    let keys = normalize_keys(keys.to_vec())?;
    let mut records = Vec::with_capacity(keys.len());
    for (index, secret) in keys.into_iter().enumerate() {
        let is_primary = index == 0;
        let existing = if is_primary {
            previous.iter().find(|record| record.is_primary)
        } else {
            previous.iter().find(|record| {
                !record.is_primary && record.secret.expose_secret() == secret.expose_secret()
            })
        };
        records.push(StoredKey {
            id: existing.map_or_else(new_key_id, |record| record.id.clone()),
            secret,
            is_primary,
        });
    }
    persist_key_records(&records)
}

pub(super) fn set_primary_api_key(key: SecretString) -> anyhow::Result<()> {
    set_primary_api_key_with_info(key).map(|_| ())
}

pub(super) fn set_primary_api_key_with_info(
    key: SecretString,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
    }
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let mut records = load_key_records()?;
    replace_primary_record(&mut records, key)?;
    persist_key_records(&records)?;
    api_key_info_for_records(&records)
}

pub(super) fn add_secondary_api_key(key: SecretString) -> anyhow::Result<()> {
    add_secondary_api_key_with_info(key).map(|_| ())
}

pub(super) fn add_secondary_api_key_with_info(
    key: SecretString,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
    }
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let mut records = load_key_records()?;
    add_secondary_record(&mut records, key)?;
    persist_key_records(&records)?;
    api_key_info_for_records(&records)
}

pub(super) fn remove_secondary_api_key(id: &str) -> anyhow::Result<()> {
    remove_secondary_api_key_with_info(id).map(|_| ())
}

pub(super) fn remove_secondary_api_key_with_info(
    id: &str,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let mut records = load_key_records()?;
    remove_secondary_record(&mut records, id)?;
    persist_key_records(&records)?;
    api_key_info_for_records(&records)
}

pub(super) fn clear_api_keys() -> anyhow::Result<()> {
    clear_api_keys_with_info().map(|_| ())
}

pub(super) fn clear_api_keys_with_info() -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    persist_key_records(&[])?;
    Ok(Vec::new())
}

pub(super) fn api_key_info() -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let records = load_key_records()?;
    api_key_info_for_records(&records)
}

fn api_key_info_for_records(records: &[StoredKey]) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let now = Instant::now();
    let mut rotation = rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))?;
    rotation.cooldowns.retain(|(_, until)| *until > now);
    Ok(records
        .iter()
        .map(|record| {
            let (status, retry_after_seconds) = key_status(&rotation, &record.secret, now);
            GeminiApiKeyInfo {
                id: record.id.clone(),
                is_primary: record.is_primary,
                status,
                retry_after_seconds,
            }
        })
        .collect())
}

fn key_status(
    rotation: &KeyRotation,
    secret: &SecretString,
    now: Instant,
) -> (GeminiApiKeyStatus, Option<u32>) {
    if rotation
        .invalid
        .iter()
        .any(|invalid| invalid.expose_secret() == secret.expose_secret())
    {
        return (GeminiApiKeyStatus::Rejected, None);
    }
    if let Some((_, until)) = rotation
        .cooldowns
        .iter()
        .find(|(cooling, _)| cooling.expose_secret() == secret.expose_secret())
    {
        return (
            GeminiApiKeyStatus::CoolingDown,
            Some(
                u32::try_from(until.saturating_duration_since(now).as_secs().max(1))
                    .unwrap_or(u32::MAX),
            ),
        );
    }
    (GeminiApiKeyStatus::Available, None)
}

pub(super) async fn test_api_key(id: &str) -> anyhow::Result<GeminiApiKeyTestResult> {
    let key = {
        let _guard = key_store_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
        load_key_records()?
            .into_iter()
            .find(|record| record.id == id)
            .map(|record| record.secret)
            .context("Gemini API key was not found")?
    };
    test_key(&key).await
}

async fn test_key(key: &SecretString) -> anyhow::Result<GeminiApiKeyTestResult> {
    let client = norrix_runtime::http_client()?;
    let url = Url::parse(ROOT).expect("Gemini API root is valid");
    let response = match client
        .get(url)
        .query(&[("pageSize", "1")])
        .header("x-goog-api-key", key.expose_secret())
        .timeout(KEY_TEST_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(GeminiApiKeyTestResult::Unavailable),
    };
    let status = response.status();
    if status.is_success() {
        return match response.json::<ModelsResponse>().await {
            Ok(_) => Ok(GeminiApiKeyTestResult::Verified),
            Err(_) => Ok(GeminiApiKeyTestResult::Unavailable),
        };
    }
    let body = response.text().await.unwrap_or_default();
    let failure = classify_http_failure(status, None, &body, true);
    Ok(match failure.action {
        RetryAction::RejectKey => GeminiApiKeyTestResult::Rejected,
        RetryAction::Retry | RetryAction::Stop => GeminiApiKeyTestResult::Unavailable,
    })
}

fn new_key_id() -> String {
    Uuid::new_v4().to_string()
}

fn parse_additional_records(value: &str) -> anyhow::Result<(Vec<StoredKey>, bool)> {
    let stored: StoredAdditionalKeys =
        serde_json::from_str(value).context("stored Gemini key collection is invalid")?;
    Ok(match stored {
        StoredAdditionalKeys::Legacy(keys) => (
            keys.into_iter()
                .map(|secret| StoredKey {
                    id: new_key_id(),
                    secret,
                    is_primary: false,
                })
                .collect(),
            true,
        ),
        StoredAdditionalKeys::Managed(keys) => (
            keys.into_iter()
                .map(|record| StoredKey {
                    id: record.id,
                    secret: record.key,
                    is_primary: false,
                })
                .collect(),
            false,
        ),
    })
}

fn serialize_additional_records(additional: &[&StoredKey]) -> anyhow::Result<Option<String>> {
    if additional.is_empty() {
        return Ok(None);
    }
    let encoded: Vec<_> = additional
        .iter()
        .map(|record| StoredAdditionalKeyRef {
            id: &record.id,
            key: record.secret.expose_secret(),
        })
        .collect();
    Ok(Some(serde_json::to_string(&encoded)?))
}

fn replace_primary_record(records: &mut Vec<StoredKey>, key: SecretString) -> anyhow::Result<()> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
    }
    if let Some(index) = records.iter().position(|record| record.is_primary) {
        let id = records[index].id.clone();
        records[index] = StoredKey {
            id,
            secret: key,
            is_primary: true,
        };
    } else {
        records.insert(
            0,
            StoredKey {
                id: new_key_id(),
                secret: key,
                is_primary: true,
            },
        );
    }
    if let Some(primary) = records.iter().find(|record| record.is_primary) {
        let primary_secret = primary.secret.expose_secret().to_owned();
        records
            .retain(|record| record.is_primary || record.secret.expose_secret() != primary_secret);
    }
    if records.len() > MAX_API_KEYS {
        anyhow::bail!("at most {MAX_API_KEYS} Gemini API keys can be configured");
    }
    Ok(())
}

fn add_secondary_record(records: &mut Vec<StoredKey>, key: SecretString) -> anyhow::Result<()> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
    }
    if records.len() >= MAX_API_KEYS {
        anyhow::bail!("at most {MAX_API_KEYS} Gemini API keys can be configured");
    }
    if records
        .iter()
        .any(|record| record.secret.expose_secret() == key.expose_secret())
    {
        anyhow::bail!("this Gemini API key is already configured");
    }
    records.push(StoredKey {
        id: new_key_id(),
        secret: key,
        is_primary: false,
    });
    Ok(())
}

fn remove_secondary_record(records: &mut Vec<StoredKey>, id: &str) -> anyhow::Result<()> {
    let Some(index) = records.iter().position(|record| record.id == id) else {
        anyhow::bail!("Gemini API key was not found");
    };
    if records[index].is_primary {
        anyhow::bail!("the primary Gemini API key cannot be removed as a secondary key");
    }
    records.remove(index);
    Ok(())
}

fn load_key_records() -> anyhow::Result<Vec<StoredKey>> {
    let primary = norrix_secrets::get("gemini")?;
    let stored_primary_id = norrix_secrets::get(PRIMARY_KEY_ID_SECRET)?;
    let mut records = Vec::new();
    let mut migrate = false;
    if let Some(primary) = primary {
        let id = stored_primary_id
            .as_ref()
            .map(|id| id.expose_secret().to_owned())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                migrate = true;
                new_key_id()
            });
        records.push(StoredKey {
            id: id.clone(),
            secret: primary,
            is_primary: true,
        });
    } else if stored_primary_id.is_some() {
        migrate = true;
    }

    let additional = norrix_secrets::get(ADDITIONAL_KEYS_SECRET)?;
    if let Some(additional) = additional {
        let (additional, legacy) = parse_additional_records(additional.expose_secret())?;
        migrate |= legacy;
        records.extend(additional);
    }
    if records.len() > MAX_API_KEYS {
        anyhow::bail!("stored Gemini key collection exceeds the supported limit");
    }
    let mut unique = Vec::<StoredKey>::new();
    for record in records {
        if unique
            .iter()
            .any(|existing| existing.secret.expose_secret() == record.secret.expose_secret())
        {
            migrate = true;
            continue;
        }
        let mut record = record;
        if record.id.is_empty() || unique.iter().any(|existing| existing.id == record.id) {
            record.id = new_key_id();
            migrate = true;
        }
        unique.push(record);
    }
    if migrate {
        persist_key_records(&unique)?;
    }
    Ok(unique)
}

fn persist_key_records(records: &[StoredKey]) -> anyhow::Result<()> {
    let mut primary: Option<&StoredKey> = None;
    let mut additional = Vec::new();
    for record in records {
        if record.is_primary {
            if primary.replace(record).is_some() {
                anyhow::bail!("Gemini key collection has multiple primary keys");
            }
        } else {
            additional.push(record);
        }
    }
    if records.len() > MAX_API_KEYS {
        anyhow::bail!("at most {MAX_API_KEYS} Gemini API keys can be configured");
    }
    let previous = [
        ("gemini", norrix_secrets::get("gemini")?),
        (
            PRIMARY_KEY_ID_SECRET,
            norrix_secrets::get(PRIMARY_KEY_ID_SECRET)?,
        ),
        (
            ADDITIONAL_KEYS_SECRET,
            norrix_secrets::get(ADDITIONAL_KEYS_SECRET)?,
        ),
    ];
    let serialized_additional = serialize_additional_records(&additional)?;
    let result = (|| {
        if let Some(primary) = primary {
            norrix_secrets::set("gemini", &primary.secret)?;
            norrix_secrets::set(
                PRIMARY_KEY_ID_SECRET,
                &SecretString::from(primary.id.clone()),
            )?;
        } else {
            norrix_secrets::delete("gemini")?;
            norrix_secrets::delete(PRIMARY_KEY_ID_SECRET)?;
        }
        if let Some(serialized) = serialized_additional {
            norrix_secrets::set(ADDITIONAL_KEYS_SECRET, &SecretString::from(serialized))?;
        } else {
            norrix_secrets::delete(ADDITIONAL_KEYS_SECRET)?;
        }
        Ok::<(), anyhow::Error>(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (key, value) in &previous {
            if let Err(rollback_error) = restore_secret(key, value.as_ref()) {
                rollback_errors.push(rollback_error.to_string());
            }
        }
        if rollback_errors.is_empty() {
            return Err(error)
                .context("failed to update Gemini API keys; previous credentials were restored");
        }
        anyhow::bail!(
            "failed to update Gemini API keys ({error}); rollback failed for {} keyring entries",
            rollback_errors.len()
        );
    }
    reset_rotation_state()?;
    Ok(())
}

fn restore_secret(key: &str, value: Option<&SecretString>) -> anyhow::Result<()> {
    if let Some(value) = value {
        norrix_secrets::set(key, value)
    } else {
        norrix_secrets::delete(key)
    }
}

fn reset_rotation_state() -> anyhow::Result<()> {
    *rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))? =
        KeyRotation::default();
    Ok(())
}

impl KeyRotation {
    fn ordered_keys(&mut self, keys: &[SecretString], now: Instant) -> anyhow::Result<Vec<ApiKey>> {
        self.cooldowns.retain(|(_, until)| *until > now);
        if keys.is_empty() {
            anyhow::bail!("Gemini API key is not configured");
        }
        let start = self.next_index % keys.len();
        self.next_index = (start + 1) % keys.len();
        let ordered: Vec<_> = (0..keys.len())
            .map(|offset| &keys[(start + offset) % keys.len()])
            .filter_map(|key| {
                let available = !self
                    .invalid
                    .iter()
                    .any(|invalid| invalid.expose_secret() == key.expose_secret())
                    && !self
                        .cooldowns
                        .iter()
                        .any(|(cooling, _)| cooling.expose_secret() == key.expose_secret());
                available.then(|| ApiKey {
                    secret: key.clone(),
                })
            })
            .collect();
        if ordered.is_empty() {
            let retry = self
                .cooldowns
                .iter()
                .map(|(_, until)| until)
                .min()
                .map(|until| until.saturating_duration_since(now).as_secs().max(1));
            let message = if keys.iter().all(|key| {
                self.invalid
                    .iter()
                    .any(|invalid| invalid.expose_secret() == key.expose_secret())
            }) {
                "all configured Gemini API keys were rejected".to_owned()
            } else if let Some(seconds) = retry {
                format!("all available Gemini API keys are cooling down; retry in {seconds}s")
            } else {
                "no Gemini API key is currently available".to_owned()
            };
            anyhow::bail!(message);
        }
        Ok(ordered)
    }

    fn record_failure(&mut self, key: &SecretString, failure: &AttemptFailure, now: Instant) {
        match failure.action {
            RetryAction::RejectKey => {
                if !self
                    .invalid
                    .iter()
                    .any(|invalid| invalid.expose_secret() == key.expose_secret())
                {
                    self.invalid.push(key.clone());
                }
            }
            RetryAction::Retry => {
                if let Some(duration) = failure.cooldown {
                    if let Some((_, until)) = self
                        .cooldowns
                        .iter_mut()
                        .find(|(cooling, _)| cooling.expose_secret() == key.expose_secret())
                    {
                        *until = now + duration;
                    } else {
                        self.cooldowns.push((key.clone(), now + duration));
                    }
                }
            }
            RetryAction::Stop => {}
        }
    }

    fn record_success(&mut self, key: &SecretString) {
        self.invalid
            .retain(|invalid| invalid.expose_secret() != key.expose_secret());
        self.cooldowns
            .retain(|(cooling, _)| cooling.expose_secret() != key.expose_secret());
    }
}

async fn execute_with_keys<T, F, Fut>(
    state: &Mutex<KeyRotation>,
    keys: Vec<SecretString>,
    mut attempt: F,
) -> anyhow::Result<T>
where
    F: FnMut(SecretString) -> Fut,
    Fut: Future<Output = std::result::Result<T, AttemptFailure>>,
{
    let ordered = {
        let mut state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?;
        state.ordered_keys(&keys, Instant::now())?
    };
    let mut failures = Vec::new();
    for key in ordered {
        match attempt(key.secret.clone()).await {
            Ok(value) => {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?
                    .record_success(&key.secret);
                return Ok(value);
            }
            Err(failure) => {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?
                    .record_failure(&key.secret, &failure, Instant::now());
                failures.push(failure.reason.clone());
                match failure.action {
                    RetryAction::Retry | RetryAction::RejectKey => continue,
                    RetryAction::Stop => {
                        anyhow::bail!("Gemini request was not retried: {}", failure.reason);
                    }
                }
            }
        }
    }
    anyhow::bail!(
        "Gemini failed after trying {} API key(s): {}",
        failures.len(),
        failures.join("; ")
    )
}

async fn send_with_keys<T, F>(
    client: &Client,
    keys: Vec<SecretString>,
    idempotent: bool,
    build: F,
) -> Result<T>
where
    T: DeserializeOwned,
    F: Fn(&Client, &str) -> RequestBuilder,
{
    Ok(execute_with_keys(rotation_state(), keys, |key| {
        send_json_gemini(build(client, key.expose_secret()), idempotent)
    })
    .await?)
}

async fn send_json_gemini<T: DeserializeOwned>(
    request: RequestBuilder,
    idempotent: bool,
) -> std::result::Result<T, AttemptFailure> {
    let response = request.send().await.map_err(|error| {
        classify_transport_failure(error.is_connect(), error.is_timeout(), idempotent)
    })?;
    let status = response.status();
    let retry_after = retry_after(response.headers(), SystemTime::now());
    if !status.is_success() {
        let text = match response.text().await {
            Ok(text) => text,
            Err(_) if status == StatusCode::TOO_MANY_REQUESTS => String::new(),
            Err(_) => {
                return Err(AttemptFailure {
                    action: RetryAction::Stop,
                    reason: format!("HTTP {} error response could not be read", status.as_u16()),
                    cooldown: None,
                });
            }
        };
        return Err(classify_http_failure(
            status,
            retry_after,
            &text,
            idempotent,
        ));
    }
    response.json().await.map_err(|_| AttemptFailure {
        action: RetryAction::Stop,
        reason: "successful response could not be decoded".to_owned(),
        cooldown: None,
    })
}

fn classify_transport_failure(
    is_connect: bool,
    is_timeout: bool,
    idempotent: bool,
) -> AttemptFailure {
    if is_connect {
        AttemptFailure {
            action: RetryAction::Retry,
            reason: "connection failed before request delivery".to_owned(),
            cooldown: None,
        }
    } else if is_timeout {
        AttemptFailure {
            action: if idempotent {
                RetryAction::Retry
            } else {
                RetryAction::Stop
            },
            reason: if idempotent {
                "idempotent request timed out".to_owned()
            } else {
                "request timed out; the result may be unknown".to_owned()
            },
            cooldown: None,
        }
    } else {
        AttemptFailure {
            action: RetryAction::Stop,
            reason: "request failed after it may have been sent".to_owned(),
            cooldown: None,
        }
    }
}

fn classify_http_failure(
    status: StatusCode,
    retry_after: Option<Duration>,
    body: &str,
    idempotent: bool,
) -> AttemptFailure {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let api_error = value.get("error");
    let api_status = api_error
        .and_then(|error| error.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let message = api_error
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let invalid_key = status == StatusCode::UNAUTHORIZED
        || api_status.eq_ignore_ascii_case("UNAUTHENTICATED")
        || message.contains("api key not valid")
        || message.contains("api key is invalid")
        || message.contains("api key was revoked")
        || message.contains("api key expired")
        || message.contains("api key has been disabled");
    if invalid_key {
        return AttemptFailure {
            action: RetryAction::RejectKey,
            reason: format!("HTTP {} rejected the API key", status.as_u16()),
            cooldown: None,
        };
    }
    let quota = status == StatusCode::TOO_MANY_REQUESTS
        || api_status.eq_ignore_ascii_case("RESOURCE_EXHAUSTED");
    if quota {
        return AttemptFailure {
            action: RetryAction::Retry,
            reason: format!("HTTP {} rate limit or quota exceeded", status.as_u16()),
            cooldown: retry_after.or_else(|| retry_info_delay(&value)),
        };
    }
    let temporary_get_failure =
        idempotent && (status == StatusCode::REQUEST_TIMEOUT || status.is_server_error());
    if temporary_get_failure {
        return AttemptFailure {
            action: RetryAction::Retry,
            reason: format!("temporary HTTP {} server failure", status.as_u16()),
            cooldown: retry_after,
        };
    }
    let code = if api_status.len() <= 64
        && api_status.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        }) {
        format!(" ({api_status})")
    } else {
        String::new()
    };
    AttemptFailure {
        action: RetryAction::Stop,
        reason: format!("HTTP {} rejected the request{code}", status.as_u16()),
        cooldown: None,
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()?
        .duration_since(now)
        .ok()
}

fn retry_info_delay(value: &serde_json::Value) -> Option<Duration> {
    value
        .get("error")?
        .get("details")?
        .as_array()?
        .iter()
        .find(|detail| {
            detail
                .get("@type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind.ends_with(".RetryInfo"))
        })?
        .get("retryDelay")?
        .as_str()
        .and_then(parse_retry_delay)
}

fn parse_retry_delay(value: &str) -> Option<Duration> {
    let value = value.strip_suffix('s')?;
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = seconds.parse::<u64>().ok()?;
    let nanos = if fraction.is_empty() {
        0
    } else {
        let padded = format!("{fraction:0<9}");
        padded.parse::<u32>().ok()?
    };
    Some(Duration::new(seconds, nanos))
}

pub(super) async fn models(client: &Client) -> Result<Vec<Model>> {
    let keys = stored_api_keys()?;
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = Url::parse(ROOT).expect("Gemini API root is valid");
    url.query_pairs_mut().append_pair("pageSize", "1000");
    let response: ModelsResponse = send_with_keys(client, keys, true, |client, key| {
        client.get(url.clone()).header("x-goog-api-key", key)
    })
    .await?;
    Ok(response
        .models
        .into_iter()
        .filter(|model| {
            model
                .supported_generation_methods
                .iter()
                .any(|method| method == "generateContent")
                && supports_translation(&model.name)
        })
        .filter_map(|model| {
            model.name.strip_prefix("models/").map(|id| Model {
                provider: Provider::Gemini,
                model: Some(id.to_owned()),
                name: model.display_name,
                quantizations: Vec::new(),
                vision: true,
                reasoning: model.thinking,
            })
        })
        .collect())
}

fn supports_translation(id: &str) -> bool {
    ![
        "antigravity",
        "computer-use",
        "deep-research",
        "embedding",
        "image",
        "imagen",
        "live",
        "lyria",
        "native-audio",
        "omni",
        "robotics",
        "tts",
        "veo",
    ]
    .iter()
    .any(|marker| id.contains(marker))
}

pub(super) async fn translate(
    client: &Client,
    _config: &GeminiConfig,
    model: &str,
    generation: &TranslationGeneration,
    request: &TranslationRequest,
) -> Result<Vec<String>> {
    let keys = stored_api_keys().context("failed to load Gemini API keys")?;
    if keys.is_empty() {
        return Err(anyhow::anyhow!("Gemini API key is not configured").into());
    }
    let (system, user) = prompt::prompts(request)?;
    let schema = prompt::output_schema(request.segments.len());
    let url =
        Url::parse(&format!("{ROOT}/{model}:generateContent")).expect("Gemini API root is valid");
    let body = Request {
        system_instruction: Content::text(&system),
        contents: [Content::user(&user, request.image.as_deref())?],
        generation_config: GenerationConfig {
            temperature: generation.temperature,
            max_output_tokens: generation.max_tokens,
            thinking_config: generation.reasoning.map(|enabled| ThinkingConfig {
                thinking_budget: model.starts_with("gemini-2.5").then(|| {
                    if enabled {
                        -1
                    } else if model.starts_with("gemini-2.5-pro") {
                        // Gemini 2.5 Pro cannot disable thinking; 128 is its minimum budget.
                        128
                    } else {
                        0
                    }
                }),
                thinking_level: (!model.starts_with("gemini-2.5")).then_some(if enabled {
                    "high"
                } else if model.starts_with("gemma-4") {
                    "minimal"
                } else {
                    "low"
                }),
            }),
            response_mime_type: "application/json",
            response_json_schema: schema,
        },
    };
    let response: Response = send_with_keys(client, keys, false, |client, key| {
        client
            .post(url.clone())
            .header("x-goog-api-key", key)
            .json(&body)
    })
    .await?;
    let text = response
        .candidates
        .into_iter()
        .next()
        .and_then(|candidate| candidate.content.parts.into_iter().next())
        .context("Gemini returned no candidate content")?
        .text;
    Ok(prompt::translations("gemini", &text, &request.segments)?)
}

#[derive(Serialize)]
struct Request<'a> {
    system_instruction: Content<'a>,
    contents: [Content<'a>; 1],
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct Content<'a> {
    parts: Vec<Part<'a>>,
}

impl<'a> Content<'a> {
    fn text(text: &'a str) -> Self {
        Self {
            parts: vec![Part::Text { text }],
        }
    }

    fn user(text: &'a str, image: Option<&image::DynamicImage>) -> anyhow::Result<Self> {
        let mut parts = vec![Part::Text { text }];
        if let Some(image) = image {
            parts.push(Part::InlineData {
                inline_data: InlineData {
                    mime_type: "image/jpeg",
                    data: encode_image(image)?.data,
                },
            });
        }
        Ok(Self { parts })
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Part<'a> {
    Text {
        text: &'a str,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: &'static str,
    data: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
    response_mime_type: &'static str,
    response_json_schema: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
}

#[derive(Deserialize)]
struct Response {
    candidates: Vec<Candidate>,
}

#[derive(Deserialize)]
struct Candidate {
    content: ResponseContent,
}

#[derive(Deserialize)]
struct ResponseContent {
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize)]
struct ResponsePart {
    text: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ListedModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedModel {
    name: String,
    display_name: String,
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    thinking: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_keys(values: &[&str]) -> Vec<SecretString> {
        values
            .iter()
            .map(|value| SecretString::from(*value))
            .collect()
    }

    fn stored_key(id: &str, value: &str, is_primary: bool) -> StoredKey {
        StoredKey {
            id: id.to_owned(),
            secret: SecretString::from(value),
            is_primary,
        }
    }

    fn retry_failure(cooldown: Option<Duration>) -> AttemptFailure {
        AttemptFailure {
            action: RetryAction::Retry,
            reason: "temporary retryable test failure".to_owned(),
            cooldown,
        }
    }

    #[test]
    fn serializes_structured_output_configuration() {
        let config = GenerationConfig {
            temperature: None,
            max_output_tokens: None,
            thinking_config: None,
            response_mime_type: "application/json",
            response_json_schema: prompt::output_schema(2),
        };
        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["responseMimeType"], "application/json");
        assert!(value.get("thinkingConfig").is_none());
        assert_eq!(
            value["responseJsonSchema"]["properties"]["translations"]["items"]["properties"]["id"]
                ["maximum"],
            1
        );
    }

    #[test]
    fn serializes_model_specific_thinking_controls() {
        let enabled = serde_json::to_value(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some("high"),
        })
        .unwrap();
        assert_eq!(enabled["thinkingLevel"], "high");
        assert!(enabled.get("thinkingBudget").is_none());

        let disabled = serde_json::to_value(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some("minimal"),
        })
        .unwrap();
        assert_eq!(disabled["thinkingLevel"], "minimal");
        assert!(disabled.get("thinkingBudget").is_none());

        let budget = serde_json::to_value(ThinkingConfig {
            thinking_budget: Some(0),
            thinking_level: None,
        })
        .unwrap();
        assert_eq!(budget["thinkingBudget"], 0);
        assert!(budget.get("thinkingLevel").is_none());
    }

    #[test]
    fn serializes_text_and_inline_image_parts() {
        let content =
            Content::user("translate", Some(&image::DynamicImage::new_rgb8(1, 1))).unwrap();
        let value = serde_json::to_value(content).unwrap();
        assert_eq!(value["parts"][0]["text"], "translate");
        assert_eq!(value["parts"][1]["inlineData"]["mimeType"], "image/jpeg");
        assert!(value["parts"][1]["inlineData"]["data"].is_string());
    }

    #[test]
    fn filters_specialized_generate_content_models() {
        assert!(supports_translation("models/gemini-3.7-flash"));
        assert!(supports_translation("models/gemma-4-31b-it"));
        assert!(!supports_translation("models/gemini-3.1-flash-image"));
        assert!(!supports_translation("models/gemini-3.1-flash-tts-preview"));
    }

    #[test]
    fn reads_optional_thinking_capability_from_model_list() {
        let response: ModelsResponse = serde_json::from_value(serde_json::json!({
            "models": [
                {
                    "name": "models/gemini-3.7-flash",
                    "displayName": "Gemini 3.7 Flash",
                    "supportedGenerationMethods": ["generateContent"],
                    "thinking": true
                },
                {
                    "name": "models/gemini-2.0-flash-lite",
                    "displayName": "Gemini 2.0 Flash-Lite",
                    "supportedGenerationMethods": ["generateContent"]
                }
            ]
        }))
        .unwrap();
        assert!(response.models[0].thinking);
        assert!(!response.models[1].thinking);
    }

    #[tokio::test]
    async fn successful_key_is_used_without_failover_and_rotation_is_round_robin() {
        let state = Mutex::new(KeyRotation::default());
        let keys = test_keys(&["test-key-a", "test-key-b"]);
        let first = execute_with_keys(&state, keys.clone(), |key| async move {
            Ok::<_, AttemptFailure>(key.expose_secret().to_owned())
        })
        .await
        .unwrap();
        let second = execute_with_keys(&state, keys, |key| async move {
            Ok::<_, AttemptFailure>(key.expose_secret().to_owned())
        })
        .await
        .unwrap();
        assert_eq!(
            [first.as_str(), second.as_str()],
            ["test-key-a", "test-key-b"]
        );
    }

    #[tokio::test]
    async fn recoverable_failure_fails_over_once_to_another_key() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys(&state, test_keys(&["test-key-a", "test-key-b"]), |key| {
            calls.borrow_mut().push(key.expose_secret().to_owned());
            async move {
                if key.expose_secret() == "test-key-a" {
                    Err(retry_failure(Some(Duration::from_secs(30))))
                } else {
                    Ok("success")
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "success");
        assert_eq!(&*calls.borrow(), &["test-key-a", "test-key-b"]);
    }

    #[tokio::test]
    async fn permanent_request_failure_does_not_rotate() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys(&state, test_keys(&["test-key-a", "test-key-b"]), |_| {
            calls.set(calls.get() + 1);
            async {
                Err::<(), _>(AttemptFailure {
                    action: RetryAction::Stop,
                    reason: "invalid request".to_owned(),
                    cooldown: None,
                })
            }
        })
        .await;
        assert!(result.unwrap_err().to_string().contains("invalid request"));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn invalid_key_is_rejected_permanently_and_never_reused() {
        let state = Mutex::new(KeyRotation::default());
        let first = execute_with_keys(
            &state,
            test_keys(&["test-key-a", "test-key-b"]),
            |key| async move {
                if key.expose_secret() == "test-key-a" {
                    Err(AttemptFailure {
                        action: RetryAction::RejectKey,
                        reason: "API key rejected".to_owned(),
                        cooldown: None,
                    })
                } else {
                    Ok(key.expose_secret().to_owned())
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(first, "test-key-b");

        let second = execute_with_keys(
            &state,
            test_keys(&["test-key-a", "test-key-b"]),
            |key| async move { Ok(key.expose_secret().to_owned()) },
        )
        .await
        .unwrap();
        assert_eq!(second, "test-key-b");
    }

    #[tokio::test]
    async fn maximum_attempts_are_limited_to_the_configured_keys() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys(
            &state,
            test_keys(&["test-key-a", "test-key-b", "test-key-c"]),
            |_| {
                calls.set(calls.get() + 1);
                async { Err::<(), _>(retry_failure(None)) }
            },
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("after trying 3 API key")
        );
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn quota_cooldown_prevents_immediate_reuse_when_every_key_is_limited() {
        let state = Mutex::new(KeyRotation::default());
        let keys = test_keys(&["test-key-a", "test-key-b"]);
        let result = execute_with_keys(&state, keys.clone(), |_| async {
            Err::<(), _>(retry_failure(Some(Duration::from_secs(30))))
        })
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("temporary retryable")
        );

        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys(&state, keys, |_| {
            calls.set(calls.get() + 1);
            async { Ok::<_, AttemptFailure>(()) }
        })
        .await;
        assert!(result.unwrap_err().to_string().contains("cooling down"));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn cooling_key_becomes_available_after_its_retry_delay() {
        let mut state = KeyRotation::default();
        let key = SecretString::from("test-key-a");
        let now = Instant::now();
        state.record_failure(&key, &retry_failure(Some(Duration::from_secs(5))), now);
        assert!(state.ordered_keys(std::slice::from_ref(&key), now).is_err());
        assert_eq!(
            state
                .ordered_keys(std::slice::from_ref(&key), now + Duration::from_secs(6))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn key_status_reflects_cooldowns_and_explicit_rejections() {
        let key = SecretString::from("fake-key-status");
        let now = Instant::now();
        let mut state = KeyRotation::default();
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Available, None)
        ));

        state.record_failure(&key, &retry_failure(Some(Duration::from_secs(8))), now);
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::CoolingDown, Some(8))
        ));

        state.record_failure(
            &key,
            &AttemptFailure {
                action: RetryAction::RejectKey,
                reason: "explicit rejection".to_owned(),
                cooldown: None,
            },
            now,
        );
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Rejected, None)
        ));
        state.record_success(&key);
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Available, None)
        ));
    }

    #[tokio::test]
    async fn concurrent_requests_reserve_round_robin_starts_without_holding_lock() {
        let state = Arc::new(Mutex::new(KeyRotation::default()));
        let keys = test_keys(&["test-key-a", "test-key-b"]);
        let first_state = Arc::clone(&state);
        let second_state = Arc::clone(&state);
        let first_keys = keys.clone();
        let first = tokio::spawn(async move {
            execute_with_keys(&first_state, first_keys, |key| async move {
                tokio::task::yield_now().await;
                Ok::<_, AttemptFailure>(key.expose_secret().to_owned())
            })
            .await
            .unwrap()
        });
        let second = tokio::spawn(async move {
            execute_with_keys(&second_state, keys, |key| async move {
                tokio::task::yield_now().await;
                Ok::<_, AttemptFailure>(key.expose_secret().to_owned())
            })
            .await
            .unwrap()
        });
        let mut results = [first.await.unwrap(), second.await.unwrap()];
        results.sort();
        assert_eq!(results, ["test-key-a", "test-key-b"]);
    }

    #[test]
    fn classifies_quota_invalid_key_and_invalid_request_separately() {
        let quota = classify_http_failure(
            StatusCode::TOO_MANY_REQUESTS,
            Some(Duration::from_secs(3)),
            "{}",
            false,
        );
        assert_eq!(quota.action, RetryAction::Retry);
        assert_eq!(quota.cooldown, Some(Duration::from_secs(3)));

        let invalid_key = classify_http_failure(
            StatusCode::UNAUTHORIZED,
            None,
            r#"{"error":{"status":"UNAUTHENTICATED"}}"#,
            false,
        );
        assert_eq!(invalid_key.action, RetryAction::RejectKey);

        let invalid_request = classify_http_failure(
            StatusCode::BAD_REQUEST,
            None,
            r#"{"error":{"status":"INVALID_ARGUMENT"}}"#,
            false,
        );
        assert_eq!(invalid_request.action, RetryAction::Stop);
    }

    #[test]
    fn classifies_transport_failures_by_delivery_safety() {
        assert_eq!(
            classify_transport_failure(true, false, false).action,
            RetryAction::Retry
        );
        assert_eq!(
            classify_transport_failure(false, true, false).action,
            RetryAction::Stop
        );
        assert_eq!(
            classify_transport_failure(false, true, true).action,
            RetryAction::Retry
        );
        assert_eq!(
            classify_transport_failure(false, false, false).action,
            RetryAction::Stop
        );
    }

    #[test]
    fn ambiguous_translation_server_failure_is_not_replayed() {
        let failure = classify_http_failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            None,
            r#"{"error":{"status":"INTERNAL"}}"#,
            false,
        );
        assert_eq!(failure.action, RetryAction::Stop);
        assert_eq!(
            classify_http_failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                r#"{"error":{"status":"INTERNAL"}}"#,
                true
            )
            .action,
            RetryAction::Retry
        );
    }

    #[test]
    fn api_error_messages_never_echo_api_key_content() {
        let failure = classify_http_failure(
            StatusCode::BAD_REQUEST,
            None,
            r#"{"error":{"status":"INVALID_ARGUMENT","message":"test-secret should not be exposed"}}"#,
            false,
        );
        assert!(!failure.reason.contains("test-secret"));
        assert!(failure.reason.contains("INVALID_ARGUMENT"));
    }

    #[test]
    fn honors_retry_after_and_google_retry_info_delays() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "12".parse().unwrap());
        assert_eq!(
            retry_after(&headers, SystemTime::now()),
            Some(Duration::from_secs(12))
        );
        assert_eq!(
            parse_retry_delay("1.250s"),
            Some(Duration::from_millis(1250))
        );
        let body = serde_json::json!({
            "error": {
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.RetryInfo",
                    "retryDelay": "2.5s"
                }]
            }
        });
        assert_eq!(retry_info_delay(&body), Some(Duration::from_millis(2500)));

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            "Sun, 06 Nov 1994 08:49:37 GMT".parse().unwrap(),
        );
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_700);
        assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(77)));
    }

    #[test]
    fn secrets_are_sent_in_a_header_not_the_request_url() {
        let request = Client::new()
            .get("https://example.test/models?pageSize=1000")
            .header("x-goog-api-key", "test-secret")
            .build()
            .unwrap();
        assert_eq!(request.url().query(), Some("pageSize=1000"));
        assert_eq!(request.headers()["x-goog-api-key"], "test-secret");
    }

    #[test]
    fn replacing_primary_preserves_its_id_and_all_secondary_keys() {
        let mut records = vec![
            stored_key("primary-id", "old-primary", true),
            stored_key("secondary-id", "secondary-secret", false),
        ];
        replace_primary_record(&mut records, SecretString::from("new-primary")).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "primary-id");
        assert_eq!(records[0].secret.expose_secret(), "new-primary");
        assert_eq!(records[1].id, "secondary-id");
        assert_eq!(records[1].secret.expose_secret(), "secondary-secret");
    }

    #[test]
    fn removing_one_secondary_preserves_primary_and_remaining_keys() {
        let mut records = vec![
            stored_key("primary-id", "primary-secret", true),
            stored_key("secondary-a", "secondary-secret-a", false),
            stored_key("secondary-b", "secondary-secret-b", false),
        ];
        remove_secondary_record(&mut records, "secondary-a").unwrap();

        assert_eq!(
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["primary-id", "secondary-b"]
        );
        assert!(remove_secondary_record(&mut records, "primary-id").is_err());
    }

    #[test]
    fn secondary_limit_and_duplicate_keys_are_enforced() {
        let mut records = (0..MAX_API_KEYS)
            .map(|index| {
                stored_key(
                    &format!("key-{index}"),
                    &format!("fake-gemini-key-{index}"),
                    index == 0,
                )
            })
            .collect::<Vec<_>>();
        assert!(
            add_secondary_record(&mut records, SecretString::from("another-fake-key")).is_err()
        );

        records.pop();
        let existing = records[0].secret.clone();
        assert!(add_secondary_record(&mut records, existing).is_err());
        add_secondary_record(&mut records, SecretString::from("fake-new-key")).unwrap();
        assert_eq!(records.len(), MAX_API_KEYS);
    }

    #[test]
    fn managed_key_metadata_never_serializes_the_secret_value() {
        let info = GeminiApiKeyInfo {
            id: "opaque-stable-id".to_owned(),
            is_primary: false,
            status: GeminiApiKeyStatus::Available,
            retry_after_seconds: None,
        };
        let encoded = serde_json::to_string(&info).unwrap();
        assert!(!encoded.contains("fake-gemini-key"));
        assert!(encoded.contains("opaque-stable-id"));
    }

    #[test]
    fn legacy_additional_key_arrays_migrate_without_changing_key_values() {
        let (records, legacy) =
            parse_additional_records(r#"["fake-key-one","fake-key-two"]"#).unwrap();
        assert!(legacy);
        assert_eq!(records.len(), 2);
        assert_ne!(records[0].id, records[1].id);
        assert_eq!(records[0].secret.expose_secret(), "fake-key-one");
        assert_eq!(records[1].secret.expose_secret(), "fake-key-two");

        let json = serialize_additional_records(&[&records[0], &records[1]])
            .unwrap()
            .unwrap();
        assert!(json.contains("fake-key-one"));
        let (reloaded, legacy) = parse_additional_records(&json).unwrap();
        assert!(!legacy);
        assert_eq!(reloaded[0].id, records[0].id);
        assert_eq!(reloaded[1].id, records[1].id);
    }
}
