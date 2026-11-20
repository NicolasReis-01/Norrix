// Ported from:
// https://github.com/koharu-rs/koharu/blob/f4ce03999ed1ae2faaec938dd52c2f41a87d03d9/crates/koharu-llm/src/providers/gemini.rs
// Model discovery:
// https://ai.google.dev/api/models

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;
use norrix_secrets::{ExposeSecret, SecretString};
use reqwest::{Client, RequestBuilder, StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::Instrument as _;
use url::Url;
use uuid::Uuid;

use crate::{
    GenerationConfig as TranslationGeneration, Model, Provider, Result, TranslationRequest,
    backend::encode_image, prompt,
};

const ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const LEGACY_PRIMARY_KEY_SECRET: &str = "gemini";
const LEGACY_PRIMARY_KEY_ID_SECRET: &str = "gemini-primary-key-id";
const ADDITIONAL_KEYS_SECRET: &str = "gemini-rotation-keys";
const ADDITIONAL_KEY_PREFIX: &str = "gemini-rotation-key-";
const ADDITIONAL_KEY_MANIFEST: &str = "gemini_rotation_keys";
const KEY_MANIFEST_VERSION: u32 = 1;
const KEY_TEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_KEY_FAILOVER_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(default)]
pub struct GeminiConfig {}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct GeminiApiKeyInfo {
    pub id: String,
    #[specta(type = f64)]
    pub index: usize,
    pub status: GeminiApiKeyStatus,
    pub retry_after_seconds: Option<u32>,
    pub manually_set: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GeminiKeyFailure {
    #[specta(type = f64)]
    pub key_index: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum GeminiRotationEvent {
    KeyAttempting {
        #[specta(type = f64)]
        key_index: usize,
        #[specta(type = Vec<f64>)]
        keys_left: Vec<usize>,
    },
    KeyFailed {
        failure: GeminiKeyFailure,
        #[specta(type = Vec<f64>)]
        keys_left: Vec<usize>,
    },
    Succeeded {
        #[specta(type = f64)]
        key_index: usize,
    },
    Exhausted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GeminiApiKeyStatus {
    Available,
    CoolingDown,
    RpmCooldown,
    InactiveDailyQuota,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GeminiApiKeyTestResult {
    Verified,
    Rejected,
    Unavailable,
    DailyQuota,
    RpmLimited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum GeminiApiKeyManualStatus {
    InactiveDailyQuota,
    RpmCooldown,
}

#[derive(Clone)]
struct StoredKey {
    id: String,
    secret: SecretString,
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

#[cfg(test)]
#[derive(Serialize)]
struct StoredAdditionalKeyRef<'a> {
    id: &'a str,
    key: &'a str,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
struct StoredAdditionalKeyManifest {
    version: u32,
    initialized: bool,
    ids: Vec<String>,
}

trait GeminiCredentialStore {
    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>>;
    fn set(&self, key: &str, secret: &SecretString) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
    fn load_manifest(&self) -> anyhow::Result<StoredAdditionalKeyManifest>;
    fn save_manifest(&self, manifest: &StoredAdditionalKeyManifest) -> anyhow::Result<()>;
}

struct SystemGeminiCredentialStore;

impl GeminiCredentialStore for SystemGeminiCredentialStore {
    fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        norrix_secrets::get(key)
    }

    fn set(&self, key: &str, secret: &SecretString) -> anyhow::Result<()> {
        norrix_secrets::set(key, secret)
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        norrix_secrets::delete(key)
    }

    fn load_manifest(&self) -> anyhow::Result<StoredAdditionalKeyManifest> {
        let config = norrix_config::load::<StoredAdditionalKeyManifest>(ADDITIONAL_KEY_MANIFEST)?;
        Ok(config.read()?.clone())
    }

    fn save_manifest(&self, manifest: &StoredAdditionalKeyManifest) -> anyhow::Result<()> {
        let config = norrix_config::load::<StoredAdditionalKeyManifest>(ADDITIONAL_KEY_MANIFEST)?;
        let mut current = config.write()?;
        *current = manifest.clone();
        current.save()?;
        Ok(())
    }
}

fn additional_key_secret_name(id: &str) -> String {
    format!("{ADDITIONAL_KEY_PREFIX}{id}")
}

#[derive(Clone)]
struct ApiKey {
    index: usize,
    secret: SecretString,
}

#[derive(Default)]
struct KeyRotation {
    cooldowns: Vec<(SecretString, Instant)>,
    rpm_cooldowns: Vec<SecretString>,
    daily_quota_inactive: Vec<SecretString>,
    manual_statuses: Vec<(SecretString, GeminiApiKeyManualStatus, Option<Instant>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetryAction {
    Retry,
    RejectKey,
    Stop,
}

#[derive(Debug)]
struct AttemptFailure {
    action: RetryAction,
    reason: String,
    cooldown: Option<Duration>,
}

static KEY_ROTATION: OnceLock<Mutex<KeyRotation>> = OnceLock::new();
static KEY_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ROTATION_EVENTS: OnceLock<tokio::sync::broadcast::Sender<GeminiRotationEvent>> =
    OnceLock::new();

pub(super) fn subscribe_rotation_events() -> tokio::sync::broadcast::Receiver<GeminiRotationEvent> {
    ROTATION_EVENTS
        .get_or_init(|| tokio::sync::broadcast::channel(32).0)
        .subscribe()
}

fn emit_rotation_event(event: GeminiRotationEvent) {
    if let Some(events) = ROTATION_EVENTS.get() {
        let _ = events.send(event);
    }
}

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
    let configured_count = keys.len();
    let discarded_empty_count = keys
        .iter()
        .filter(|key| key.expose_secret().trim().is_empty())
        .count();
    let keys = normalize_keys(keys.to_vec())?;
    let discarded_duplicate_count = configured_count
        .saturating_sub(discarded_empty_count)
        .saturating_sub(keys.len());
    tracing::info!(
        event = "gemini_keys_configured",
        configured_count,
        valid_key_count = configured_count.saturating_sub(discarded_empty_count),
        valid_unique_count = keys.len(),
        discarded_empty_count,
        discarded_duplicate_count,
        result = "stored",
    );
    if discarded_empty_count > 0 || discarded_duplicate_count > 0 {
        tracing::warn!(
            event = "gemini_keys_discarded",
            empty_count = discarded_empty_count,
            duplicate_count = discarded_duplicate_count,
            result = "discarded",
        );
    }
    let mut records = Vec::with_capacity(keys.len());
    for secret in keys {
        let existing = previous
            .iter()
            .find(|record| record.secret.expose_secret() == secret.expose_secret());
        records.push(StoredKey {
            id: existing.map_or_else(new_key_id, |record| record.id.clone()),
            secret,
        });
    }
    persist_key_records(&records)
}

pub(super) fn add_api_key(key: SecretString) -> anyhow::Result<()> {
    add_api_key_with_info(key).map(|_| ())
}

pub(super) fn add_api_key_with_info(key: SecretString) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
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

pub(super) fn replace_api_key_with_info(
    id: &str,
    key: SecretString,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
    }
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let mut records = load_key_records()?;
    let Some(index) = records.iter().position(|record| record.id == id) else {
        anyhow::bail!("Gemini API key was not found");
    };
    if records
        .iter()
        .any(|existing| existing.id != id && existing.secret.expose_secret() == key.expose_secret())
    {
        anyhow::bail!("this Gemini API key is already configured");
    }
    records[index].secret = key;
    persist_key_records(&records)?;
    api_key_info_for_records(&records)
}

pub(super) fn remove_api_key_with_info(id: &str) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let _guard = key_store_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
    let mut records = load_key_records()?;
    let Some(index) = records.iter().position(|record| record.id == id) else {
        anyhow::bail!("Gemini API key was not found");
    };
    records.remove(index);
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

pub(super) fn set_api_key_manual_status(
    id: &str,
    status: Option<GeminiApiKeyManualStatus>,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let records = {
        let _guard = key_store_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
        load_key_records()?
    };
    let record = records
        .iter()
        .find(|record| record.id == id)
        .context("Gemini API key was not found")?;
    let mut rotation = rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))?;
    rotation.clear_status(&record.secret);
    if let Some(status) = status {
        rotation.manual_statuses.push((
            record.secret.clone(),
            status,
            (status == GeminiApiKeyManualStatus::RpmCooldown)
                .then(|| Instant::now() + Duration::from_secs(60)),
        ));
    }
    api_key_info_for_records_with_state(&records, &mut rotation)
}

fn api_key_info_for_records(records: &[StoredKey]) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let mut rotation = rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))?;
    api_key_info_for_records_with_state(records, &mut rotation)
}

fn api_key_info_for_records_with_state(
    records: &[StoredKey],
    rotation: &mut KeyRotation,
) -> anyhow::Result<Vec<GeminiApiKeyInfo>> {
    let now = Instant::now();
    rotation.prune_expired(now);
    let key_info = records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let (status, retry_after_seconds, manually_set) =
                key_status(rotation, &record.secret, now);
            GeminiApiKeyInfo {
                id: record.id.clone(),
                index: index + 1,
                status,
                retry_after_seconds,
                manually_set,
            }
        })
        .collect::<Vec<_>>();
    tracing::info!(
        event = "gemini_key_availability",
        loaded_unique_count = records.len(),
        valid_unique_count = records
            .iter()
            .filter(|record| !record.secret.expose_secret().trim().is_empty())
            .count(),
        available_count = key_info
            .iter()
            .filter(|key| matches!(key.status, GeminiApiKeyStatus::Available))
            .count(),
        cooling_down_count = key_info
            .iter()
            .filter(|key| {
                matches!(
                    key.status,
                    GeminiApiKeyStatus::CoolingDown | GeminiApiKeyStatus::RpmCooldown
                )
            })
            .count(),
        inactive_daily_quota_count = key_info
            .iter()
            .filter(|key| matches!(key.status, GeminiApiKeyStatus::InactiveDailyQuota))
            .count(),
        result = "reported",
    );
    Ok(key_info)
}

fn key_status(
    rotation: &KeyRotation,
    secret: &SecretString,
    now: Instant,
) -> (GeminiApiKeyStatus, Option<u32>, bool) {
    if let Some((_, status, until)) = rotation
        .manual_statuses
        .iter()
        .find(|(manual, _, _)| manual.expose_secret() == secret.expose_secret())
    {
        return match status {
            GeminiApiKeyManualStatus::InactiveDailyQuota => {
                (GeminiApiKeyStatus::InactiveDailyQuota, None, true)
            }
            GeminiApiKeyManualStatus::RpmCooldown => (
                GeminiApiKeyStatus::RpmCooldown,
                until.map(|until| remaining_seconds(until, now)),
                true,
            ),
        };
    }
    if rotation
        .daily_quota_inactive
        .iter()
        .any(|inactive| inactive.expose_secret() == secret.expose_secret())
    {
        return (GeminiApiKeyStatus::InactiveDailyQuota, None, false);
    }
    let rpm_cooldown = rotation
        .rpm_cooldowns
        .iter()
        .any(|cooling| cooling.expose_secret() == secret.expose_secret());
    if let Some((_, until)) = rotation
        .cooldowns
        .iter()
        .find(|(cooling, _)| cooling.expose_secret() == secret.expose_secret())
    {
        return (
            if rpm_cooldown {
                GeminiApiKeyStatus::RpmCooldown
            } else {
                GeminiApiKeyStatus::CoolingDown
            },
            Some(remaining_seconds(*until, now)),
            false,
        );
    }
    (GeminiApiKeyStatus::Available, None, false)
}

fn remaining_seconds(until: Instant, now: Instant) -> u32 {
    u32::try_from(until.saturating_duration_since(now).as_secs().max(1)).unwrap_or(u32::MAX)
}

pub(super) async fn test_api_key(id: &str) -> anyhow::Result<GeminiApiKeyTestResult> {
    let (key, records) = {
        let _guard = key_store_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("Gemini key storage is unavailable"))?;
        let records = load_key_records()?;
        let key = records
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.secret.clone())
            .context("Gemini API key was not found")?;
        (key, records)
    };
    let result = test_key(&key).await?;
    let mut rotation = rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))?;
    rotation.record_test_result(&key, result, Instant::now());
    drop(rotation);
    let _ = api_key_info_for_records(&records)?;
    Ok(result)
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
    Ok(test_result_for_failure(&failure))
}

fn test_result_for_failure(failure: &AttemptFailure) -> GeminiApiKeyTestResult {
    match failure.action {
        RetryAction::RejectKey => GeminiApiKeyTestResult::Rejected,
        RetryAction::Retry if failure.reason.contains("(requests_per_day)") => {
            GeminiApiKeyTestResult::DailyQuota
        }
        RetryAction::Retry if failure.reason.contains("(requests_per_minute)") => {
            GeminiApiKeyTestResult::RpmLimited
        }
        RetryAction::Retry | RetryAction::Stop => GeminiApiKeyTestResult::Unavailable,
    }
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
                })
                .collect(),
            true,
        ),
        StoredAdditionalKeys::Managed(keys) => (
            keys.into_iter()
                .map(|record| StoredKey {
                    id: record.id,
                    secret: record.key,
                })
                .collect(),
            false,
        ),
    })
}

#[cfg(test)]
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

fn add_secondary_record(records: &mut Vec<StoredKey>, key: SecretString) -> anyhow::Result<()> {
    if key.expose_secret().trim().is_empty() {
        anyhow::bail!("Gemini API key cannot be empty");
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
    });
    Ok(())
}

fn load_key_records() -> anyhow::Result<Vec<StoredKey>> {
    load_key_records_with_store(&SystemGeminiCredentialStore)
}

fn load_key_records_with_store(
    store: &impl GeminiCredentialStore,
) -> anyhow::Result<Vec<StoredKey>> {
    let manifest = store.load_manifest()?;
    let mut records = Vec::new();
    let migrate = if manifest.version == KEY_MANIFEST_VERSION {
        for id in &manifest.ids {
            let secret_name = additional_key_secret_name(id);
            let secret = store.get(&secret_name)?.with_context(|| {
                format!("Gemini API key {id} is listed but its credential is missing")
            })?;
            records.push(StoredKey {
                id: id.clone(),
                secret,
            });
        }
        false
    } else {
        if manifest.version > KEY_MANIFEST_VERSION {
            anyhow::bail!("Gemini API key manifest version is newer than this application");
        }
        if let Some(primary) = store.get(LEGACY_PRIMARY_KEY_SECRET)? {
            let id = store
                .get(LEGACY_PRIMARY_KEY_ID_SECRET)?
                .map(|value| value.expose_secret().to_owned())
                .filter(|id| valid_key_id(id))
                .unwrap_or_else(new_key_id);
            records.push(StoredKey {
                id,
                secret: primary,
            });
        }
        if manifest.initialized {
            for id in &manifest.ids {
                let secret = store
                    .get(&additional_key_secret_name(id))?
                    .with_context(|| {
                        format!("Gemini API key {id} is listed but its credential is missing")
                    })?;
                records.push(StoredKey {
                    id: id.clone(),
                    secret,
                });
            }
        } else if let Some(additional) = store.get(ADDITIONAL_KEYS_SECRET)? {
            let (additional, _) = parse_additional_records(additional.expose_secret())?;
            records.extend(additional);
        }
        true
    };

    let configured_count = records.len();
    let mut unique = Vec::<StoredKey>::new();
    let mut changed = migrate;
    for mut record in records {
        if unique
            .iter()
            .any(|existing| existing.secret.expose_secret() == record.secret.expose_secret())
        {
            changed = true;
            continue;
        }
        if !valid_key_id(&record.id) || unique.iter().any(|existing| existing.id == record.id) {
            record.id = new_key_id();
            changed = true;
        }
        unique.push(record);
    }
    if changed {
        persist_key_records_with_store(store, &unique)?;
    }
    let valid_unique_count = unique
        .iter()
        .filter(|record| !record.secret.expose_secret().trim().is_empty())
        .count();
    tracing::info!(
        event = "gemini_keys_loaded",
        configured_count,
        valid_unique_count,
        loaded_unique_count = unique.len(),
        discarded_duplicate_count = configured_count.saturating_sub(unique.len()),
        migration_applied = changed,
        result = "loaded",
    );
    if valid_unique_count < unique.len() {
        tracing::warn!(
            event = "gemini_keys_invalid",
            invalid_empty_count = unique.len().saturating_sub(valid_unique_count),
            result = "retained_for_safe_reconfiguration",
        );
    }
    Ok(unique)
}

fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn persist_key_records(records: &[StoredKey]) -> anyhow::Result<()> {
    persist_key_records_with_store(&SystemGeminiCredentialStore, records)
}

fn persist_key_records_with_store(
    store: &impl GeminiCredentialStore,
    records: &[StoredKey],
) -> anyhow::Result<()> {
    let previous_primary = store.get(LEGACY_PRIMARY_KEY_SECRET)?;
    let previous_primary_id = store.get(LEGACY_PRIMARY_KEY_ID_SECRET)?;
    let previous_legacy = store.get(ADDITIONAL_KEYS_SECRET)?;
    let previous_manifest = store.load_manifest()?;
    let next_manifest = StoredAdditionalKeyManifest {
        version: KEY_MANIFEST_VERSION,
        initialized: true,
        ids: records.iter().map(|record| record.id.clone()).collect(),
    };
    let mut credential_names = previous_manifest
        .ids
        .iter()
        .chain(next_manifest.ids.iter())
        .map(|id| additional_key_secret_name(id))
        .collect::<Vec<_>>();
    credential_names.sort();
    credential_names.dedup();
    if !previous_manifest.initialized {
        for record in records {
            credential_names.push(additional_key_secret_name(&record.id));
        }
        credential_names.sort();
        credential_names.dedup();
    }
    let previous_credentials = credential_names
        .iter()
        .map(|name| Ok((name.clone(), store.get(name)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let result = (|| {
        for record in records {
            store.set(&additional_key_secret_name(&record.id), &record.secret)?;
        }
        store.save_manifest(&next_manifest)?;
        for id in &previous_manifest.ids {
            if !next_manifest.ids.contains(id) {
                store.delete(&additional_key_secret_name(id))?;
            }
        }
        store.delete(LEGACY_PRIMARY_KEY_SECRET)?;
        store.delete(LEGACY_PRIMARY_KEY_ID_SECRET)?;
        store.delete(ADDITIONAL_KEYS_SECRET)?;
        Ok::<(), anyhow::Error>(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (key, value) in &[
            (LEGACY_PRIMARY_KEY_SECRET, previous_primary),
            (LEGACY_PRIMARY_KEY_ID_SECRET, previous_primary_id),
            (ADDITIONAL_KEYS_SECRET, previous_legacy),
        ] {
            if let Err(rollback_error) = restore_secret(store, key, value.as_ref()) {
                rollback_errors.push(rollback_error.to_string());
            }
        }
        for (name, value) in &previous_credentials {
            if let Err(rollback_error) = restore_secret(store, name, value.as_ref()) {
                rollback_errors.push(rollback_error.to_string());
            }
        }
        if let Err(rollback_error) = store.save_manifest(&previous_manifest) {
            rollback_errors.push(rollback_error.to_string());
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
    reconcile_rotation_keys(records)?;
    Ok(())
}

fn restore_secret(
    store: &impl GeminiCredentialStore,
    key: &str,
    value: Option<&SecretString>,
) -> anyhow::Result<()> {
    if let Some(value) = value {
        store.set(key, value)
    } else {
        store.delete(key)
    }
}

fn reconcile_rotation_keys(records: &[StoredKey]) -> anyhow::Result<()> {
    let mut rotation = rotation_state()
        .lock()
        .map_err(|_| anyhow::anyhow!("Gemini rotation state is unavailable"))?;
    rotation.retain_configured_keys(records);
    Ok(())
}

impl KeyRotation {
    fn ordered_keys(&mut self, keys: &[SecretString], now: Instant) -> anyhow::Result<Vec<ApiKey>> {
        self.prune_expired(now);
        if keys.is_empty() {
            anyhow::bail!("Gemini API key is not configured");
        }
        let ordered = keys
            .iter()
            .enumerate()
            .filter(|(_, key)| !self.is_unavailable(key, now))
            .map(|(index, key)| ApiKey {
                index: index + 1,
                secret: key.clone(),
            })
            .collect::<Vec<_>>();
        if ordered.is_empty() {
            anyhow::bail!("all configured Gemini API keys are temporarily unavailable");
        }
        Ok(ordered)
    }

    fn record_failure(&mut self, key: &SecretString, failure: &AttemptFailure, now: Instant) {
        if self.is_manually_set(key) {
            return;
        }
        if failure.reason.contains("(requests_per_day)") {
            if !self.contains_secret(&self.daily_quota_inactive, key) {
                self.daily_quota_inactive.push(key.clone());
            }
            self.remove_cooldown(key);
            return;
        }
        if failure.reason.contains("(requests_per_minute)") {
            self.set_cooldown(key, now + Duration::from_secs(60), true);
            return;
        }
        match failure.action {
            RetryAction::Retry => {
                if let Some(duration) = failure.cooldown {
                    self.set_cooldown(key, now + duration, false);
                }
            }
            RetryAction::RejectKey | RetryAction::Stop => {}
        }
    }

    fn record_test_result(
        &mut self,
        key: &SecretString,
        result: GeminiApiKeyTestResult,
        now: Instant,
    ) {
        if self.is_manually_set(key) {
            return;
        }
        match result {
            GeminiApiKeyTestResult::Verified => {
                self.remove_cooldown(key);
                self.remove_daily_quota(key);
            }
            GeminiApiKeyTestResult::DailyQuota => {
                if !self.contains_secret(&self.daily_quota_inactive, key) {
                    self.daily_quota_inactive.push(key.clone());
                }
                self.remove_cooldown(key);
            }
            GeminiApiKeyTestResult::RpmLimited => {
                self.set_cooldown(key, now + Duration::from_secs(60), true);
            }
            GeminiApiKeyTestResult::Rejected | GeminiApiKeyTestResult::Unavailable => {}
        }
    }

    fn record_success(&mut self, key: &SecretString) {
        if !self.is_manually_set(key) {
            self.remove_cooldown(key);
        }
    }

    fn prune_expired(&mut self, now: Instant) {
        self.cooldowns.retain(|(_, until)| *until > now);
        self.rpm_cooldowns.retain(|cooling| {
            self.cooldowns
                .iter()
                .any(|(key, until)| key.expose_secret() == cooling.expose_secret() && *until > now)
        });
        self.manual_statuses.retain(|(_, status, until)| {
            *status != GeminiApiKeyManualStatus::RpmCooldown
                || until.is_some_and(|until| until > now)
        });
    }

    fn is_unavailable(&self, key: &SecretString, now: Instant) -> bool {
        self.is_manually_set(key)
            || self.contains_secret(&self.daily_quota_inactive, key)
            || self.cooldowns.iter().any(|(cooling, until)| {
                cooling.expose_secret() == key.expose_secret() && *until > now
            })
    }

    fn is_manually_set(&self, key: &SecretString) -> bool {
        self.manual_statuses
            .iter()
            .any(|(manual, _, _)| manual.expose_secret() == key.expose_secret())
    }

    fn contains_secret(&self, keys: &[SecretString], key: &SecretString) -> bool {
        keys.iter()
            .any(|candidate| candidate.expose_secret() == key.expose_secret())
    }

    fn clear_status(&mut self, key: &SecretString) {
        self.remove_cooldown(key);
        self.rpm_cooldowns
            .retain(|cooling| cooling.expose_secret() != key.expose_secret());
        self.remove_daily_quota(key);
        self.manual_statuses
            .retain(|(manual, _, _)| manual.expose_secret() != key.expose_secret());
    }

    fn remove_cooldown(&mut self, key: &SecretString) {
        self.cooldowns
            .retain(|(cooling, _)| cooling.expose_secret() != key.expose_secret());
        self.rpm_cooldowns
            .retain(|cooling| cooling.expose_secret() != key.expose_secret());
    }

    fn remove_daily_quota(&mut self, key: &SecretString) {
        self.daily_quota_inactive
            .retain(|inactive| inactive.expose_secret() != key.expose_secret());
    }

    fn set_cooldown(&mut self, key: &SecretString, until: Instant, rpm: bool) {
        if let Some((_, deadline)) = self
            .cooldowns
            .iter_mut()
            .find(|(cooling, _)| cooling.expose_secret() == key.expose_secret())
        {
            *deadline = until;
        } else {
            self.cooldowns.push((key.clone(), until));
        }
        self.rpm_cooldowns
            .retain(|cooling| cooling.expose_secret() != key.expose_secret());
        if rpm {
            self.rpm_cooldowns.push(key.clone());
        }
    }

    fn retain_configured_keys(&mut self, records: &[StoredKey]) {
        let has_key = |key: &SecretString| {
            records
                .iter()
                .any(|record| record.secret.expose_secret() == key.expose_secret())
        };
        self.cooldowns.retain(|(key, _)| has_key(key));
        self.rpm_cooldowns.retain(has_key);
        self.daily_quota_inactive.retain(has_key);
        self.manual_statuses.retain(|(key, _, _)| has_key(key));
    }
}

async fn execute_with_keys<T, F, Fut>(
    state: &Mutex<KeyRotation>,
    keys: Vec<SecretString>,
    attempt: F,
) -> anyhow::Result<T>
where
    F: FnMut(SecretString) -> Fut,
    Fut: Future<Output = std::result::Result<T, AttemptFailure>>,
{
    execute_with_keys_and_wait(state, keys, attempt, tokio::time::sleep).await
}

async fn execute_with_keys_and_wait<T, F, Fut, W, Wfut>(
    state: &Mutex<KeyRotation>,
    keys: Vec<SecretString>,
    attempt: F,
    wait: W,
) -> anyhow::Result<T>
where
    F: FnMut(SecretString) -> Fut,
    Fut: Future<Output = std::result::Result<T, AttemptFailure>>,
    W: FnMut(Duration) -> Wfut,
    Wfut: Future<Output = ()>,
{
    execute_with_keys_and_wait_and_emit(state, keys, attempt, wait, emit_rotation_event).await
}

async fn execute_with_keys_and_wait_and_emit<T, F, Fut, W, Wfut, E>(
    state: &Mutex<KeyRotation>,
    keys: Vec<SecretString>,
    mut attempt: F,
    mut wait: W,
    mut emit: E,
) -> anyhow::Result<T>
where
    F: FnMut(SecretString) -> Fut,
    Fut: Future<Output = std::result::Result<T, AttemptFailure>>,
    W: FnMut(Duration) -> Wfut,
    Wfut: Future<Output = ()>,
    E: FnMut(GeminiRotationEvent),
{
    let ordered = {
        let mut state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?;
        state.ordered_keys(&keys, Instant::now())?
    };
    let mut failures = Vec::new();
    let total_keys = ordered.len();
    let mut keys_tried = 0;
    let mut attempts = 0;
    for key_position in 0..ordered.len() {
        let key = ordered[key_position].clone();
        let keys_left = ordered[key_position + 1..]
            .iter()
            .map(|next| next.index)
            .collect::<Vec<_>>();
        keys_tried += 1;
        attempts += 1;
        emit(GeminiRotationEvent::KeyAttempting {
            key_index: key.index,
            keys_left: keys_left.clone(),
        });
        tracing::info!(
            event = "gemini_key_selected",
            key_index = key.index,
            configured_key_count = total_keys,
            attempt_for_key = 1,
            total_attempts = attempts,
            result = "selected",
        );
        match attempt(key.secret.clone()).await {
            Ok(value) => {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?
                    .record_success(&key.secret);
                tracing::info!(
                    event = "gemini_rotation_finished",
                    configured_key_count = total_keys,
                    keys_tried,
                    attempts,
                    successful_key_index = key.index,
                    result = "succeeded",
                );
                emit(GeminiRotationEvent::Succeeded {
                    key_index: key.index,
                });
                return Ok(value);
            }
            Err(failure) => {
                state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Gemini key rotation state is unavailable"))?
                    .record_failure(&key.secret, &failure, Instant::now());
                if failure.action == RetryAction::Stop {
                    tracing::warn!(
                        event = "gemini_request_rejected_without_rotation",
                        key_index = key.index,
                        configured_key_count = total_keys,
                        attempt_count = attempts,
                        reason = %failure.reason,
                        result = "stopped",
                    );
                    anyhow::bail!("Gemini request was not retried: {}", failure.reason);
                }

                failures.push(failure.reason.clone());
                tracing::warn!(
                    event = "gemini_rotation_key_failed",
                    key_index = key.index,
                    configured_key_count = total_keys,
                    attempt_count = attempts,
                    reason = %failure.reason,
                    next_key_index = if key_position + 1 < total_keys {
                        key.index + 1
                    } else {
                        0
                    },
                    result = "rotating",
                );
                emit(GeminiRotationEvent::KeyFailed {
                    failure: GeminiKeyFailure {
                        key_index: key.index,
                        reason: failure.reason.clone(),
                    },
                    keys_left,
                });
                tracing::info!(
                    event = "gemini_retry_scheduler_decision",
                    key_index = key.index,
                    next_key_index = if key_position + 1 < total_keys {
                        key.index + 1
                    } else {
                        0
                    },
                    configured_key_count = total_keys,
                    attempt_for_key = 1,
                    action = retry_action_name(failure.action),
                    next_step = if key_position + 1 < total_keys {
                        "try_next_key"
                    } else {
                        "exhausted"
                    },
                    retry_after_ms = failure
                        .cooldown
                        .map(|duration| duration.as_secs_f64() * 1000.0),
                    result = "decided",
                );

                if failure.action == RetryAction::Retry && key_position + 1 < total_keys {
                    let delay = failover_delay(failure.cooldown);
                    if !delay.is_zero() {
                        tracing::info!(
                            event = "gemini_key_failover_wait",
                            key_index = key.index,
                            next_key_index = key.index + 1,
                            requested_delay_ms = failure
                                .cooldown
                                .map(|duration| duration.as_secs_f64() * 1000.0),
                            applied_delay_ms = delay.as_secs_f64() * 1000.0,
                            result = "capped",
                        );
                        wait(delay).await;
                    }
                }
            }
        }
    }
    tracing::warn!(
        event = "gemini_rotation_finished",
        configured_key_count = total_keys,
        keys_tried,
        attempts,
        result = "exhausted",
    );
    emit(GeminiRotationEvent::Exhausted);
    anyhow::bail!(
        "Gemini translation failed: exhausted {keys_tried} of {total_keys} configured API keys over {attempts} attempts; {}",
        summarize_failure_reasons(&failures)
    )
}

fn summarize_failure_reasons(reasons: &[String]) -> String {
    let mut counts = BTreeMap::new();
    for reason in reasons {
        *counts.entry(reason.as_str()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(reason, count)| {
            if count == 1 {
                reason.to_owned()
            } else {
                format!("{reason} ({count} attempts)")
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn retry_action_name(action: RetryAction) -> &'static str {
    match action {
        RetryAction::Retry => "rotate_key",
        RetryAction::RejectKey => "reject_key",
        RetryAction::Stop => "stop",
    }
}

fn failover_delay(retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_default().min(MAX_KEY_FAILOVER_DELAY)
}

async fn send_with_keys<T, F>(
    client: &Client,
    keys: Vec<SecretString>,
    idempotent: bool,
    request_id: Uuid,
    endpoint: &'static str,
    model: &str,
    build: F,
) -> Result<T>
where
    T: DeserializeOwned,
    F: Fn(&Client, &str) -> RequestBuilder,
{
    let mut attempt_count = 0usize;
    let key_count = keys.len();
    let span = tracing::info_span!(
        "gemini_http_request",
        request_id = %request_id,
        endpoint,
        model,
        configured_key_count = key_count,
    );
    Ok(execute_with_keys(rotation_state(), keys, |key| {
        attempt_count += 1;
        send_json_gemini(
            build(client, key.expose_secret()),
            idempotent,
            attempt_count.saturating_sub(1),
            HttpAttemptContext {
                request_id,
                endpoint,
                model: model.to_owned(),
                attempt: attempt_count,
            },
        )
    })
    .instrument(span)
    .await?)
}

#[derive(Clone)]
struct HttpAttemptContext {
    request_id: Uuid,
    endpoint: &'static str,
    model: String,
    attempt: usize,
}

struct HttpAttemptCancellationTrace {
    context: HttpAttemptContext,
    started: Instant,
    phase: &'static str,
    finished: bool,
}

impl Drop for HttpAttemptCancellationTrace {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                event = "gemini_http_attempt_cancelled",
                request_id = %self.context.request_id,
                endpoint = self.context.endpoint,
                model = %self.context.model,
                attempt = self.context.attempt,
                cancelled_during_phase = self.phase,
                client_observed_attempt_duration_ms =
                    self.started.elapsed().as_secs_f64() * 1000.0,
                result = "cancelled",
            );
        }
    }
}

async fn send_json_gemini<T: DeserializeOwned>(
    request: RequestBuilder,
    idempotent: bool,
    retry_count: usize,
    context: HttpAttemptContext,
) -> std::result::Result<T, AttemptFailure> {
    let attempt_started = Instant::now();
    let mut cancellation_trace = HttpAttemptCancellationTrace {
        context: context.clone(),
        started: attempt_started,
        phase: "waiting_for_response_headers",
        finished: false,
    };
    tracing::info!(
        event = "gemini_http_attempt_started",
        request_id = %context.request_id,
        endpoint = context.endpoint,
        model = %context.model,
        attempt = context.attempt,
        retry_count,
        timeout_policy = "shared_client_connect_timeout_only",
        result = "started",
    );
    let headers_started = Instant::now();
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let failure =
                classify_transport_failure(error.is_connect(), error.is_timeout(), idempotent);
            tracing::error!(
                event = "gemini_http_attempt_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = "unavailable",
                client_send_to_response_headers_duration_ms =
                    headers_started.elapsed().as_secs_f64() * 1000.0,
                transport_error_is_connect = error.is_connect(),
                transport_error_is_timeout = error.is_timeout(),
                client_observed_attempt_duration_ms =
                    attempt_started.elapsed().as_secs_f64() * 1000.0,
                result = "failed",
            );
            log_retry_decision(context, &failure);
            cancellation_trace.finished = true;
            return Err(failure);
        }
    };
    let status = response.status();
    cancellation_trace.phase = "reading_response_body";
    let headers_duration_ms = headers_started.elapsed().as_secs_f64() * 1000.0;
    let retry_after = retry_after(response.headers(), SystemTime::now());
    let quota_headers = quota_headers(response.headers());
    tracing::info!(
        event = "gemini_http_response_headers_received",
        request_id = %context.request_id,
        endpoint = context.endpoint,
        model = %context.model,
        attempt = context.attempt,
        status_code = status.as_u16(),
        client_send_to_response_headers_duration_ms = headers_duration_ms,
        retry_after_ms = retry_after.map(|duration| duration.as_secs_f64() * 1000.0),
        quota_limit = quota_headers.limit,
        quota_remaining = quota_headers.remaining,
        quota_reset = quota_headers.reset,
        result = "received",
    );
    if !status.is_success() {
        let response_started = Instant::now();
        let body = match response.bytes().await {
            Ok(body) => Some(body),
            Err(_) => {
                tracing::error!(
                    event = "gemini_response_body_received",
                    request_id = %context.request_id,
                    endpoint = context.endpoint,
                    model = %context.model,
                    attempt = context.attempt,
                    retry_count,
                    status_code = status.as_u16(),
                    duration_ms = response_started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                tracing::error!(
                    event = "gemini_http_attempt_finished",
                    request_id = %context.request_id,
                    endpoint = context.endpoint,
                    model = %context.model,
                    attempt = context.attempt,
                    retry_count,
                    status_code = status.as_u16(),
                    client_send_to_response_headers_duration_ms = headers_duration_ms,
                    response_body_duration_ms =
                        response_started.elapsed().as_secs_f64() * 1000.0,
                    client_observed_attempt_duration_ms =
                        attempt_started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                None
            }
        };
        let body = body.as_deref().unwrap_or_default();
        let body_duration_ms = response_started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            event = "gemini_response_body_received",
            request_id = %context.request_id,
            endpoint = context.endpoint,
            model = %context.model,
            attempt = context.attempt,
            retry_count,
            status_code = status.as_u16(),
            body_bytes = body.len(),
            duration_ms = body_duration_ms,
            result = "received",
        );
        let decode_started = Instant::now();
        let text = String::from_utf8_lossy(body);
        let failure = classify_http_failure(status, retry_after, &text, idempotent);
        let decode_duration_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            event = "gemini_response_decoding_finished",
            request_id = %context.request_id,
            endpoint = context.endpoint,
            model = %context.model,
            attempt = context.attempt,
            retry_count,
            status_code = status.as_u16(),
            duration_ms = decode_duration_ms,
            result = "decoded_error_response",
        );
        tracing::error!(
            event = "gemini_http_attempt_finished",
            request_id = %context.request_id,
            endpoint = context.endpoint,
            model = %context.model,
            attempt = context.attempt,
            retry_count,
            status_code = status.as_u16(),
            client_send_to_response_headers_duration_ms = headers_duration_ms,
            response_body_duration_ms = body_duration_ms,
            response_decoding_duration_ms = decode_duration_ms,
            response_body_bytes = body.len(),
            client_observed_attempt_duration_ms = attempt_started.elapsed().as_secs_f64() * 1000.0,
            result = "http_error",
        );
        log_retry_decision(context, &failure);
        cancellation_trace.finished = true;
        return Err(failure);
    }
    let body_started = Instant::now();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            let body_duration_ms = body_started.elapsed().as_secs_f64() * 1000.0;
            tracing::error!(
                event = "gemini_response_body_received",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                duration_ms = body_duration_ms,
                body_read_error_is_timeout = error.is_timeout(),
                result = "failed",
            );
            tracing::error!(
                event = "gemini_http_attempt_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                client_send_to_response_headers_duration_ms = headers_duration_ms,
                response_body_duration_ms = body_duration_ms,
                client_observed_attempt_duration_ms =
                    attempt_started.elapsed().as_secs_f64() * 1000.0,
                result = "response_body_failed",
            );
            let failure = AttemptFailure {
                action: RetryAction::Retry,
                reason: "Gemini response communication failed while reading the body".to_owned(),
                cooldown: None,
            };
            log_retry_decision(context, &failure);
            cancellation_trace.finished = true;
            return Err(failure);
        }
    };
    cancellation_trace.phase = "decoding_response";
    let body_duration_ms = body_started.elapsed().as_secs_f64() * 1000.0;
    tracing::info!(
        event = "gemini_response_body_received",
        request_id = %context.request_id,
        endpoint = context.endpoint,
        model = %context.model,
        attempt = context.attempt,
        retry_count,
        status_code = status.as_u16(),
        body_bytes = body.len(),
        duration_ms = body_duration_ms,
        result = "received",
    );
    let decode_started = Instant::now();
    let decoded = serde_json::from_slice::<T>(&body);
    let decode_duration_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    match decoded {
        Ok(value) => {
            tracing::info!(
                event = "gemini_response_decoding_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                duration_ms = decode_duration_ms,
                result = "decoded",
            );
            tracing::info!(
                event = "gemini_http_attempt_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                client_send_to_response_headers_duration_ms = headers_duration_ms,
                response_body_duration_ms = body_duration_ms,
                response_decoding_duration_ms = decode_duration_ms,
                response_body_bytes = body.len(),
                client_observed_attempt_duration_ms =
                    attempt_started.elapsed().as_secs_f64() * 1000.0,
                result = "success",
            );
            cancellation_trace.finished = true;
            Ok(value)
        }
        Err(_) => {
            tracing::error!(
                event = "gemini_response_decoding_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                duration_ms = decode_duration_ms,
                result = "failed",
            );
            tracing::error!(
                event = "gemini_http_attempt_finished",
                request_id = %context.request_id,
                endpoint = context.endpoint,
                model = %context.model,
                attempt = context.attempt,
                retry_count,
                status_code = status.as_u16(),
                client_send_to_response_headers_duration_ms = headers_duration_ms,
                response_body_duration_ms = body_duration_ms,
                response_decoding_duration_ms = decode_duration_ms,
                response_body_bytes = body.len(),
                client_observed_attempt_duration_ms =
                    attempt_started.elapsed().as_secs_f64() * 1000.0,
                result = "response_decode_failed",
            );
            let failure = AttemptFailure {
                action: RetryAction::Stop,
                reason: "successful response could not be decoded".to_owned(),
                cooldown: None,
            };
            log_retry_decision(context, &failure);
            cancellation_trace.finished = true;
            Err(failure)
        }
    }
}

fn log_retry_decision(context: HttpAttemptContext, failure: &AttemptFailure) {
    tracing::info!(
        event = "gemini_retry_decision",
        request_id = %context.request_id,
        endpoint = context.endpoint,
        model = %context.model,
        attempt = context.attempt,
        action = retry_action_name(failure.action),
        cooldown_ms = failure.cooldown.map(|duration| duration.as_secs_f64() * 1000.0),
        result = "decided",
    );
}

#[derive(Default)]
struct QuotaHeaders {
    limit: Option<u64>,
    remaining: Option<u64>,
    reset: Option<u64>,
}

fn quota_headers(headers: &reqwest::header::HeaderMap) -> QuotaHeaders {
    let parse = |name| headers.get(name)?.to_str().ok()?.trim().parse().ok();
    QuotaHeaders {
        limit: parse("x-ratelimit-limit"),
        remaining: parse("x-ratelimit-remaining"),
        reset: parse("x-ratelimit-reset"),
    }
}

#[derive(Default)]
struct QuotaFailureMetadata {
    categories: Vec<&'static str>,
    values: Vec<u64>,
    limits: Vec<u64>,
}

fn quota_failure_metadata(value: &serde_json::Value) -> QuotaFailureMetadata {
    let mut metadata = QuotaFailureMetadata::default();
    let Some(details) = value
        .get("error")
        .and_then(|error| error.get("details"))
        .and_then(serde_json::Value::as_array)
    else {
        return metadata;
    };

    for detail in details {
        let is_quota_failure = detail
            .get("@type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind.ends_with(".QuotaFailure"));
        if !is_quota_failure {
            continue;
        }
        let Some(violations) = detail
            .get("violations")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for violation in violations {
            let metric = violation
                .get("quotaMetric")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let id = violation
                .get("quotaId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let category = classify_quota_category(metric, id);
            if !metadata.categories.contains(&category) {
                metadata.categories.push(category);
            }
            if let Some(value) = numeric_metadata(violation.get("quotaValue")) {
                if !metadata.values.contains(&value) {
                    metadata.values.push(value);
                }
            }
            if let Some(limit) = numeric_metadata(violation.get("quotaLimit")) {
                if !metadata.limits.contains(&limit) {
                    metadata.limits.push(limit);
                }
            }
        }
    }
    metadata
}

fn numeric_metadata(value: Option<&serde_json::Value>) -> Option<u64> {
    value
        .and_then(serde_json::Value::as_u64)
        .or_else(|| value.and_then(serde_json::Value::as_str)?.parse().ok())
}

fn classify_quota_category(metric: &str, id: &str) -> &'static str {
    let normalized = format!("{metric} {id}").to_ascii_lowercase();
    if normalized.contains("request") && normalized.contains("perday") {
        "requests_per_day"
    } else if normalized.contains("request") && normalized.contains("perminute") {
        "requests_per_minute"
    } else if normalized.contains("token") && normalized.contains("perminute") {
        "tokens_per_minute"
    } else {
        "unknown"
    }
}

fn classify_transport_failure(
    is_connect: bool,
    is_timeout: bool,
    _idempotent: bool,
) -> AttemptFailure {
    if is_connect {
        AttemptFailure {
            action: RetryAction::Retry,
            reason: "Gemini connection failed before request delivery".to_owned(),
            cooldown: None,
        }
    } else if is_timeout {
        AttemptFailure {
            action: RetryAction::Retry,
            reason: "Gemini request timed out; provider result may be unknown".to_owned(),
            cooldown: None,
        }
    } else {
        AttemptFailure {
            action: RetryAction::Retry,
            reason: "Gemini communication failed; provider result may be unknown".to_owned(),
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
            reason: format!(
                "Gemini HTTP {} rejected the API key; verify that the configured key is valid",
                status.as_u16()
            ),
            cooldown: None,
        };
    }
    let quota = status == StatusCode::TOO_MANY_REQUESTS
        || api_status.eq_ignore_ascii_case("RESOURCE_EXHAUSTED");
    if quota {
        let retry_info = retry_info_delay(&value);
        let cooldown = retry_after.or(retry_info);
        let metadata = quota_failure_metadata(&value);
        let categories = if metadata.categories.is_empty() {
            vec!["unknown"]
        } else {
            metadata.categories.clone()
        };
        tracing::info!(
            event = "gemini_cooldown_source_selected",
            status_code = status.as_u16(),
            source = if retry_after.is_some() {
                "retry_after"
            } else if retry_info.is_some() {
                "retry_info"
            } else {
                "none"
            },
            cooldown_ms = cooldown.map(|duration| duration.as_secs_f64() * 1000.0),
            result = "classified",
        );
        tracing::warn!(
            event = "gemini_quota_failure_classified",
            categories = ?categories,
            quota_values = ?metadata.values,
            quota_limits = ?metadata.limits,
            retry_info_ms = retry_info.map(|duration| duration.as_secs_f64() * 1000.0),
            result = "classified",
        );
        return AttemptFailure {
            action: RetryAction::Retry,
            reason: format!(
                "Gemini HTTP {} quota exceeded ({})",
                status.as_u16(),
                categories.join(", ")
            ),
            cooldown,
        };
    }
    let temporary_failure =
        status.is_server_error() || (idempotent && status == StatusCode::REQUEST_TIMEOUT);
    if temporary_failure {
        return AttemptFailure {
            action: RetryAction::Retry,
            reason: format!(
                "Gemini HTTP {} server error; retryable failure",
                status.as_u16()
            ),
            cooldown: retry_after.or_else(|| retry_info_delay(&value)),
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
    if status == StatusCode::FORBIDDEN {
        return AttemptFailure {
            action: RetryAction::Retry,
            reason: format!("Gemini HTTP 403 project or permission access denied{code}"),
            cooldown: None,
        };
    }
    AttemptFailure {
        action: RetryAction::Stop,
        reason: format!("Gemini HTTP {} rejected the request{code}", status.as_u16()),
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
    let response: ModelsResponse = send_with_keys(
        client,
        keys,
        true,
        Uuid::new_v4(),
        "model_list",
        "model-list",
        |client, key| client.get(url.clone()).header("x-goog-api-key", key),
    )
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
    config: &GeminiConfig,
    model: &str,
    generation: &TranslationGeneration,
    request: &TranslationRequest,
) -> Result<Vec<String>> {
    let started = Instant::now();
    let request_id = Uuid::new_v4();
    tracing::info!(
        event = "gemini_provider_request_started",
        request_id = %request_id,
        provider = "gemini",
        model,
        segment_count = request.segments.len(),
        result = "started",
    );
    let result = translate_inner(client, config, model, generation, request, request_id).await;
    match &result {
        Ok(_) => tracing::info!(
            event = "gemini_provider_end_to_end_finished",
            request_id = %request_id,
            provider = "gemini",
            model,
            segment_count = request.segments.len(),
            provider_end_to_end_duration_ms = started.elapsed().as_secs_f64() * 1000.0,
            result = "translated",
        ),
        Err(_) => tracing::error!(
            event = "gemini_provider_end_to_end_finished",
            request_id = %request_id,
            provider = "gemini",
            model,
            segment_count = request.segments.len(),
            provider_end_to_end_duration_ms = started.elapsed().as_secs_f64() * 1000.0,
            result = "failed",
        ),
    }
    result
}

async fn translate_inner(
    client: &Client,
    _config: &GeminiConfig,
    model: &str,
    generation: &TranslationGeneration,
    request: &TranslationRequest,
    request_id: Uuid,
) -> Result<Vec<String>> {
    let keys = stored_api_keys().context("failed to load Gemini API keys")?;
    if keys.is_empty() {
        return Err(anyhow::anyhow!("Gemini API key is not configured").into());
    }
    let prompt_started = Instant::now();
    let (system, user) = prompt::prompts(request)?;
    tracing::info!(
        event = "translation_prompt_constructed",
        request_id = %request_id,
        provider = "gemini",
        model,
        segment_count = request.segments.len(),
        duration_ms = prompt_started.elapsed().as_secs_f64() * 1000.0,
        result = "constructed",
    );

    let request_started = Instant::now();
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
    tracing::info!(
        event = "translation_provider_request_constructed",
        request_id = %request_id,
        provider = "gemini",
        model,
        segment_count = request.segments.len(),
        duration_ms = request_started.elapsed().as_secs_f64() * 1000.0,
        result = "constructed",
    );
    let response: Response = send_with_keys(
        client,
        keys,
        false,
        request_id,
        "generate_content",
        model,
        |client, key| {
            client
                .post(url.clone())
                .header("x-goog-api-key", key)
                .json(&body)
        },
    )
    .await?;
    if let Some(usage) = &response.usage_metadata {
        tracing::info!(
            event = "gemini_generation_usage",
            request_id = %request_id,
            provider = "gemini",
            model,
            prompt_tokens = usage.prompt_token_count,
            candidate_tokens = usage.candidates_token_count,
            thinking_tokens = usage.thoughts_token_count,
            total_tokens = usage.total_token_count,
            result = "reported",
        );
    } else {
        tracing::debug!(
            event = "gemini_generation_usage",
            request_id = %request_id,
            provider = "gemini",
            model,
            result = "not_provided",
        );
    }
    let extraction_started = Instant::now();
    let text = match response_text(response) {
        Ok(text) => {
            tracing::info!(
                event = "gemini_response_text_extracted",
                request_id = %request_id,
                provider = "gemini",
                model,
                duration_ms = extraction_started.elapsed().as_secs_f64() * 1000.0,
                result = "extracted",
            );
            text
        }
        Err(error) => {
            tracing::error!(
                event = "gemini_response_text_extraction_failed",
                request_id = %request_id,
                provider = "gemini",
                model,
                duration_ms = extraction_started.elapsed().as_secs_f64() * 1000.0,
                result = "failed",
            );
            return Err(error.into());
        }
    };
    let parsing_started = Instant::now();
    match prompt::translations("gemini", &text, &request.segments) {
        Ok(translations) => {
            tracing::info!(
                event = "translation_response_parsed",
                request_id = %request_id,
                provider = "gemini",
                model,
                segment_count = request.segments.len(),
                duration_ms = parsing_started.elapsed().as_secs_f64() * 1000.0,
                result = "parsed",
            );
            Ok(translations)
        }
        Err(error) => {
            tracing::error!(
                event = "translation_response_parsing_failed",
                request_id = %request_id,
                provider = "gemini",
                model,
                segment_count = request.segments.len(),
                duration_ms = parsing_started.elapsed().as_secs_f64() * 1000.0,
                result = "failed",
            );
            Err(error.into())
        }
    }
}

fn response_text(response: Response) -> anyhow::Result<String> {
    let candidate = response
        .candidates
        .into_iter()
        .next()
        .context("Gemini returned no candidate content")?;
    let text = candidate
        .content
        .parts
        .into_iter()
        .filter(|part| !part.thought)
        .filter_map(|part| part.text)
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::ensure!(
        !text.trim().is_empty(),
        "Gemini returned no non-thinking candidate text"
    );
    Ok(text)
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
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
    total_token_count: Option<u64>,
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
    #[serde(default)]
    thought: bool,
    text: Option<String>,
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
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug, Deserialize)]
    struct TestHttpResponse {
        value: String,
    }

    fn test_http_context(attempt: usize) -> HttpAttemptContext {
        HttpAttemptContext {
            request_id: Uuid::new_v4(),
            endpoint: "test",
            model: "test-model".to_owned(),
            attempt,
        }
    }

    async fn local_http_server(responses: Vec<String>) -> (Url, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let Ok(count) = stream.read(&mut buffer).await else {
                        return;
                    };
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                if stream.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
            }
        });
        (
            Url::parse(&format!("http://{address}/generateContent")).unwrap(),
            server,
        )
    }

    fn mock_response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum StoreOperation {
        Get,
        Set,
        Delete,
        LoadManifest,
        SaveManifest,
    }

    #[derive(Default)]
    struct MemoryCredentialState {
        secrets: HashMap<String, SecretString>,
        manifest: StoredAdditionalKeyManifest,
        fail_next: Option<StoreOperation>,
    }

    #[derive(Clone, Default)]
    struct MemoryCredentialStore(Arc<Mutex<MemoryCredentialState>>);

    struct TemporaryStoreDirectory(std::path::PathBuf);

    impl TemporaryStoreDirectory {
        fn create() -> anyhow::Result<Self> {
            let path = std::env::temp_dir()
                .join(format!("norrix-gemini-credential-test-{}", Uuid::new_v4()));
            std::fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TemporaryStoreDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct FileCredentialStore {
        root: std::path::PathBuf,
    }

    impl FileCredentialStore {
        fn new(root: std::path::PathBuf) -> Self {
            Self { root }
        }

        fn secret_path(&self, key: &str) -> std::path::PathBuf {
            self.root.join(format!("{key}.secret.json"))
        }

        fn read_optional(&self, path: &std::path::Path) -> anyhow::Result<Option<Vec<u8>>> {
            match std::fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
    }

    impl GeminiCredentialStore for FileCredentialStore {
        fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
            self.read_optional(&self.secret_path(key))?
                .map(|bytes| {
                    serde_json::from_slice::<String>(&bytes)
                        .map(SecretString::from)
                        .map_err(Into::into)
                })
                .transpose()
        }

        fn set(&self, key: &str, secret: &SecretString) -> anyhow::Result<()> {
            let bytes = serde_json::to_vec(secret.expose_secret())?;
            std::fs::write(self.secret_path(key), bytes)?;
            Ok(())
        }

        fn delete(&self, key: &str) -> anyhow::Result<()> {
            match std::fs::remove_file(self.secret_path(key)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }

        fn load_manifest(&self) -> anyhow::Result<StoredAdditionalKeyManifest> {
            let Some(bytes) = self.read_optional(&self.root.join("manifest.json"))? else {
                return Ok(StoredAdditionalKeyManifest::default());
            };
            Ok(serde_json::from_slice(&bytes)?)
        }

        fn save_manifest(&self, manifest: &StoredAdditionalKeyManifest) -> anyhow::Result<()> {
            std::fs::write(
                self.root.join("manifest.json"),
                serde_json::to_vec(manifest)?,
            )?;
            Ok(())
        }
    }

    impl MemoryCredentialStore {
        fn fail_next(&self, operation: StoreOperation) {
            self.0.lock().unwrap().fail_next = Some(operation);
        }

        fn take_failure(
            state: &mut MemoryCredentialState,
            operation: StoreOperation,
        ) -> anyhow::Result<()> {
            if state.fail_next == Some(operation) {
                state.fail_next = None;
                anyhow::bail!("simulated {operation:?} credential-store failure");
            }
            Ok(())
        }
    }

    impl GeminiCredentialStore for MemoryCredentialStore {
        fn get(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
            let mut state = self.0.lock().unwrap();
            Self::take_failure(&mut state, StoreOperation::Get)?;
            Ok(state.secrets.get(key).cloned())
        }

        fn set(&self, key: &str, secret: &SecretString) -> anyhow::Result<()> {
            let mut state = self.0.lock().unwrap();
            Self::take_failure(&mut state, StoreOperation::Set)?;
            state.secrets.insert(key.to_owned(), secret.clone());
            Ok(())
        }

        fn delete(&self, key: &str) -> anyhow::Result<()> {
            let mut state = self.0.lock().unwrap();
            Self::take_failure(&mut state, StoreOperation::Delete)?;
            state.secrets.remove(key);
            Ok(())
        }

        fn load_manifest(&self) -> anyhow::Result<StoredAdditionalKeyManifest> {
            let mut state = self.0.lock().unwrap();
            Self::take_failure(&mut state, StoreOperation::LoadManifest)?;
            Ok(state.manifest.clone())
        }

        fn save_manifest(&self, manifest: &StoredAdditionalKeyManifest) -> anyhow::Result<()> {
            let mut state = self.0.lock().unwrap();
            Self::take_failure(&mut state, StoreOperation::SaveManifest)?;
            state.manifest = manifest.clone();
            Ok(())
        }
    }

    fn test_keys(values: &[&str]) -> Vec<SecretString> {
        values
            .iter()
            .map(|value| SecretString::from(*value))
            .collect()
    }

    fn stored_key(id: &str, value: &str, _legacy_position: bool) -> StoredKey {
        StoredKey {
            id: id.to_owned(),
            secret: SecretString::from(value),
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
    fn deserializes_token_usage_metadata() {
        let response: Response = serde_json::from_str(
            r#"{
                "candidates": [],
                "usageMetadata": {
                    "promptTokenCount": 17,
                    "candidatesTokenCount": 8,
                    "thoughtsTokenCount": 2,
                    "totalTokenCount": 27
                }
            }"#,
        )
        .unwrap();
        let usage = response.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(17));
        assert_eq!(usage.candidates_token_count, Some(8));
        assert_eq!(usage.thoughts_token_count, Some(2));
        assert_eq!(usage.total_token_count, Some(27));
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

    #[test]
    fn response_text_skips_gemini_thought_parts_and_collects_final_text() {
        let response: Response = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "thought": true, "text": "internal reasoning, not JSON" },
                        { "text": "{\"translations\":[{\"id\":\"1\",\"text\":\"Bonjour\"}]}" }
                    ]
                }
            }]
        }))
        .unwrap();

        assert_eq!(
            response_text(response).unwrap(),
            "{\"translations\":[{\"id\":\"1\",\"text\":\"Bonjour\"}]}"
        );
    }

    #[test]
    fn response_text_rejects_responses_without_final_text() {
        let response: Response = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{ "thought": true, "text": "internal reasoning" }]
                }
            }]
        }))
        .unwrap();

        assert!(response_text(response).is_err());
    }

    #[tokio::test]
    async fn every_execution_starts_with_the_first_configured_key() {
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
            ["test-key-a", "test-key-a"]
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
    async fn rejected_key_is_retried_in_a_later_execution() {
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
        assert_eq!(second, "test-key-a");
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
                .contains("exhausted 3 of 3 configured API keys over 3 attempts")
        );
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn rpd_quota_authentication_and_connection_failures_rotate_once() {
        let rpd_body = serde_json::json!({
            "error": {
                "status": "RESOURCE_EXHAUSTED",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.QuotaFailure",
                    "violations": [{
                        "quotaMetric": "GenerateRequestsPerDayPerProjectPerModel",
                        "quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier",
                        "quotaValue": "20",
                        "quotaLimit": "20"
                    }]
                }]
            }
        });
        let cases = [
            (
                "rpd",
                classify_http_failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    Some(Duration::from_secs(86_400)),
                    &rpd_body.to_string(),
                    false,
                ),
            ),
            (
                "authentication",
                classify_http_failure(
                    StatusCode::UNAUTHORIZED,
                    None,
                    r#"{"error":{"status":"UNAUTHENTICATED"}}"#,
                    false,
                ),
            ),
            ("connection", classify_transport_failure(true, false, false)),
        ];

        for (_, first_failure) in cases {
            let state = Mutex::new(KeyRotation::default());
            let calls = std::cell::RefCell::new(Vec::new());
            let result = execute_with_keys(
                &state,
                test_keys(&["project-a-key", "project-b-key"]),
                |key| {
                    calls.borrow_mut().push(key.expose_secret().to_owned());
                    let is_first = key.expose_secret() == "project-a-key";
                    let failure = AttemptFailure {
                        action: first_failure.action,
                        reason: first_failure.reason.clone(),
                        cooldown: first_failure.cooldown,
                    };
                    async move {
                        if is_first {
                            Err(failure)
                        } else {
                            Ok("recovered")
                        }
                    }
                },
            )
            .await
            .unwrap();
            assert_eq!(result, "recovered");
            assert_eq!(&*calls.borrow(), &["project-a-key", "project-b-key"]);
        }
    }

    #[tokio::test]
    async fn four_failed_keys_rotate_to_the_fifth_success() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::RefCell::new(Vec::new());
        let keys = [
            "project-1-key",
            "project-2-key",
            "project-3-key",
            "project-4-key",
            "project-5-key",
        ];
        let result = execute_with_keys(&state, test_keys(&keys), |key| {
            calls.borrow_mut().push(key.expose_secret().to_owned());
            async move {
                if key.expose_secret() == "project-5-key" {
                    Ok("translated")
                } else {
                    Err(classify_http_failure(
                        StatusCode::TOO_MANY_REQUESTS,
                        None,
                        r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#,
                        true,
                    ))
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "translated");
        assert_eq!(
            &*calls.borrow(),
            &[
                "project-1-key",
                "project-2-key",
                "project-3-key",
                "project-4-key",
                "project-5-key",
            ]
        );
    }

    #[tokio::test]
    async fn rotation_emits_safe_ordered_events_for_app_job_updates() {
        let state = Mutex::new(KeyRotation::default());
        let events = std::cell::RefCell::new(Vec::new());
        let calls = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys_and_wait_and_emit(
            &state,
            test_keys(&["project-a-key", "project-b-key", "project-c-key"]),
            |key| {
                calls.borrow_mut().push(key.expose_secret().to_owned());
                async move {
                    if key.expose_secret() == "project-c-key" {
                        Ok("translated")
                    } else {
                        Err(AttemptFailure {
                            action: RetryAction::RejectKey,
                            reason: "Gemini HTTP 401 rejected the API key".to_owned(),
                            cooldown: None,
                        })
                    }
                }
            },
            |_| std::future::ready(()),
            |event| events.borrow_mut().push(event),
        )
        .await
        .unwrap();

        assert_eq!(result, "translated");
        assert_eq!(
            &*calls.borrow(),
            &["project-a-key", "project-b-key", "project-c-key"]
        );
        let events = events.borrow();
        let attempted = events
            .iter()
            .filter_map(|event| match event {
                GeminiRotationEvent::KeyAttempting {
                    key_index,
                    keys_left,
                } => Some((*key_index, keys_left.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(attempted, [(1, vec![2, 3]), (2, vec![3]), (3, vec![])]);
        let failures = events
            .iter()
            .filter_map(|event| match event {
                GeminiRotationEvent::KeyFailed { failure, .. } => Some(failure),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            failures
                .iter()
                .map(|failure| failure.key_index)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        for failure in failures {
            assert_eq!(failure.reason, "Gemini HTTP 401 rejected the API key");
            assert!(!failure.reason.contains("project-a-key"));
            assert!(!failure.reason.contains("project-b-key"));
            assert!(!failure.reason.contains("project-c-key"));
        }
        assert!(matches!(
            events.last(),
            Some(GeminiRotationEvent::Succeeded { key_index: 3 })
        ));
    }

    #[tokio::test]
    async fn invalid_payload_stops_without_rotating() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys(
            &state,
            test_keys(&["project-a-key", "project-b-key"]),
            |_| {
                calls.set(calls.get() + 1);
                async {
                    Err::<(), _>(classify_http_failure(
                        StatusCode::BAD_REQUEST,
                        None,
                        r#"{"error":{"status":"INVALID_ARGUMENT"}}"#,
                        true,
                    ))
                }
            },
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("INVALID_ARGUMENT"));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn http_503_advances_once_to_the_next_key() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys_and_wait(
            &state,
            test_keys(&["project-a-key", "project-b-key"]),
            |key| {
                calls.borrow_mut().push(key.expose_secret().to_owned());
                async move {
                    if key.expose_secret() == "project-a-key" {
                        Err(classify_http_failure(
                            StatusCode::SERVICE_UNAVAILABLE,
                            None,
                            r#"{"error":{"status":"UNAVAILABLE"}}"#,
                            false,
                        ))
                    } else {
                        Ok("success")
                    }
                }
            },
            |_| std::future::ready(()),
        )
        .await
        .unwrap();
        assert_eq!(result, "success");
        assert_eq!(&*calls.borrow(), &["project-a-key", "project-b-key"]);
    }

    #[tokio::test]
    async fn long_retry_after_is_capped_before_trying_the_next_key() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::RefCell::new(Vec::new());
        let delays = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys_and_wait(
            &state,
            test_keys(&["project-a-key", "project-b-key"]),
            |key| {
                calls.borrow_mut().push(key.expose_secret().to_owned());
                async move {
                    if key.expose_secret() == "project-a-key" {
                        Err(classify_http_failure(
                            StatusCode::SERVICE_UNAVAILABLE,
                            Some(Duration::from_secs(3600)),
                            r#"{"error":{"status":"UNAVAILABLE"}}"#,
                            false,
                        ))
                    } else {
                        Ok("success")
                    }
                }
            },
            |delay| {
                delays.borrow_mut().push(delay);
                std::future::ready(())
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "success");
        assert_eq!(&*calls.borrow(), &["project-a-key", "project-b-key"]);
        assert_eq!(&*delays.borrow(), &[MAX_KEY_FAILOVER_DELAY]);
        assert_eq!(
            failover_delay(Some(Duration::from_secs(3600))),
            MAX_KEY_FAILOVER_DELAY
        );
    }

    #[tokio::test]
    async fn all_five_project_failures_make_exactly_one_attempt_per_key() {
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::RefCell::new(Vec::new());
        let events = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys_and_wait_and_emit(
            &state,
            test_keys(&[
                "project-1-key",
                "project-2-key",
                "project-3-key",
                "project-4-key",
                "project-5-key",
            ]),
            |key| {
                calls.borrow_mut().push(key.expose_secret().to_owned());
                async {
                    Err::<(), _>(classify_http_failure(
                        StatusCode::SERVICE_UNAVAILABLE,
                        None,
                        r#"{"error":{"status":"UNAVAILABLE"}}"#,
                        false,
                    ))
                }
            },
            |_| std::future::ready(()),
            |event| events.borrow_mut().push(event),
        )
        .await
        .unwrap_err();
        assert!(
            result
                .to_string()
                .contains("exhausted 5 of 5 configured API keys over 5 attempts")
        );
        assert_eq!(
            &*calls.borrow(),
            &[
                "project-1-key",
                "project-2-key",
                "project-3-key",
                "project-4-key",
                "project-5-key",
            ]
        );
        assert_eq!(calls.borrow().len(), 5);
        assert_eq!(events.borrow().len(), 11);
        assert!(
            events
                .borrow()
                .iter()
                .filter_map(|event| match event {
                    GeminiRotationEvent::KeyAttempting { key_index, .. } => Some(*key_index),
                    _ => None,
                })
                .eq(1..=5)
        );
        assert!(matches!(
            events.borrow().last(),
            Some(GeminiRotationEvent::Exhausted)
        ));
    }

    #[tokio::test]
    async fn local_http_503_advances_to_the_next_key_and_succeeds() {
        let body = r#"{"value":"ok"}"#;
        let (url, server) = local_http_server(vec![
            mock_response(
                "503 Service Unavailable",
                "",
                r#"{"error":{"status":"UNAVAILABLE"}}"#,
            ),
            mock_response("200 OK", "Content-Type: application/json\r\n", body),
        ])
        .await;
        let state = Mutex::new(KeyRotation::default());
        let client = Client::new();
        let attempts = std::cell::Cell::new(0);
        let result = execute_with_keys_and_wait(
            &state,
            test_keys(&["simulated-key-1", "simulated-key-2"]),
            |key| {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                assert_eq!(
                    key.expose_secret(),
                    if attempt == 1 {
                        "simulated-key-1"
                    } else {
                        "simulated-key-2"
                    }
                );
                let request = client.get(url.clone());
                async move {
                    send_json_gemini::<TestHttpResponse>(
                        request,
                        true,
                        attempt - 1,
                        test_http_context(attempt),
                    )
                    .await
                }
            },
            |_| std::future::ready(()),
        )
        .await
        .unwrap();

        assert_eq!(result.value, "ok");
        assert_eq!(attempts.get(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_http_429_captures_retry_after_and_quota_headers() {
        let body = r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#;
        let (url, server) = local_http_server(vec![mock_response(
            "429 Too Many Requests",
            "Retry-After: 3\r\nX-RateLimit-Limit: 10\r\nX-RateLimit-Remaining: 2\r\nX-RateLimit-Reset: 12345\r\n",
            body,
        )])
        .await;
        let failure = send_json_gemini::<TestHttpResponse>(
            Client::new().get(url),
            true,
            0,
            test_http_context(1),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(failure.cooldown, Some(Duration::from_secs(3)));

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-limit", "10".parse().unwrap());
        headers.insert("x-ratelimit-remaining", "2".parse().unwrap());
        headers.insert("x-ratelimit-reset", "12345".parse().unwrap());
        let quota = quota_headers(&headers);
        assert_eq!(quota.limit, Some(10));
        assert_eq!(quota.remaining, Some(2));
        assert_eq!(quota.reset, Some(12345));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_http_all_keys_rate_limited_are_summarized_and_cooled_down() {
        let body = r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#;
        let response = mock_response("429 Too Many Requests", "Retry-After: 30\r\n", body);
        let (url, server) = local_http_server(vec![response; 2]).await;
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys_and_wait(
            &state,
            test_keys(&["test-key-a", "test-key-b"]),
            |key| {
                calls.set(calls.get() + 1);
                let attempt = calls.get();
                let client = Client::new();
                let url = url.clone();
                async move {
                    let _key = key;
                    send_json_gemini::<TestHttpResponse>(
                        client.get(url),
                        true,
                        attempt - 1,
                        test_http_context(attempt),
                    )
                    .await
                }
            },
            |_| std::future::ready(()),
        )
        .await;

        let error = result.unwrap_err().to_string();
        assert!(error.contains("exhausted 2 of 2 configured API keys over 2 attempts"));
        assert!(error.contains("Gemini HTTP 429 quota exceeded (unknown) (2 attempts)"));
        assert_eq!(calls.get(), 2);
        server.await.unwrap();

        let reused = execute_with_keys(
            &state,
            test_keys(&["test-key-a", "test-key-b"]),
            |_| async {
                calls.set(calls.get() + 1);
                Ok::<_, AttemptFailure>(())
            },
        )
        .await;
        assert!(reused.is_err());
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test]
    async fn local_http_mixed_quota_and_server_failures_are_summarized() {
        let quota_body = r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#;
        let server_body = r#"{"error":{"status":"UNAVAILABLE"}}"#;
        let responses = vec![
            mock_response("429 Too Many Requests", "", quota_body),
            mock_response("503 Service Unavailable", "", server_body),
        ];
        let (url, server) = local_http_server(responses).await;
        let state = Mutex::new(KeyRotation::default());
        let calls = std::cell::Cell::new(0);
        let result = execute_with_keys_and_wait(
            &state,
            test_keys(&["test-key-a", "test-key-b"]),
            |key| {
                calls.set(calls.get() + 1);
                let attempt = calls.get();
                let client = Client::new();
                let url = url.clone();
                async move {
                    let _key = key;
                    send_json_gemini::<TestHttpResponse>(
                        client.get(url),
                        true,
                        attempt - 1,
                        test_http_context(attempt),
                    )
                    .await
                }
            },
            |_| std::future::ready(()),
        )
        .await;

        let error = result.unwrap_err().to_string();
        assert!(error.contains("exhausted 2 of 2 configured API keys over 2 attempts"));
        assert!(error.contains("Gemini HTTP 429 quota exceeded (unknown)"));
        assert!(error.contains("Gemini HTTP 503 server error; retryable failure"));
        assert_eq!(calls.get(), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_http_body_receipt_is_measured_separately_from_json_decoding() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let body = r#"{"value":"ok"}"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body.as_bytes()[..4]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(60)).await;
            stream.write_all(&body.as_bytes()[4..]).await.unwrap();
        });
        let started = Instant::now();
        let response = send_json_gemini::<TestHttpResponse>(
            Client::new().get(format!("http://{address}/generateContent")),
            true,
            0,
            test_http_context(1),
        )
        .await
        .unwrap();
        assert_eq!(response.value, "ok");
        assert!(started.elapsed() >= Duration::from_millis(50));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_http_timeout_before_response_headers_is_reported() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
        });
        let client = Client::builder()
            .timeout(Duration::from_millis(40))
            .build()
            .unwrap();
        let failure = send_json_gemini::<TestHttpResponse>(
            client.get(format!("http://{address}/generateContent")),
            false,
            0,
            test_http_context(1),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.action, RetryAction::Retry);
        assert!(failure.reason.contains("result may be unknown"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn local_connection_failure_is_distinguished_from_http_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let failure = send_json_gemini::<TestHttpResponse>(
            Client::new().get(format!("http://{address}/generateContent")),
            false,
            0,
            test_http_context(1),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.action, RetryAction::Retry);
        assert!(
            failure
                .reason
                .contains("connection failed before request delivery")
        );
    }

    #[tokio::test]
    async fn local_http_timeout_during_body_receipt_is_reported() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let body = r#"{"value":"ok"}"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body.as_bytes()[..4]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let client = Client::builder()
            .timeout(Duration::from_millis(40))
            .build()
            .unwrap();
        let failure = send_json_gemini::<TestHttpResponse>(
            client.get(format!("http://{address}/generateContent")),
            false,
            0,
            test_http_context(1),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(
            failure.reason,
            "Gemini response communication failed while reading the body"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn empty_success_response_is_rejected() {
        let (url, server) = local_http_server(vec![mock_response("200 OK", "", "")]).await;
        let failure = send_json_gemini::<TestHttpResponse>(
            Client::new().get(url),
            false,
            0,
            test_http_context(1),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.action, RetryAction::Stop);
        assert_eq!(failure.reason, "successful response could not be decoded");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_request_while_reading_body_closes_the_http_stream() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let body = r#"{"value":"ok"}"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (response_sent, response_received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body.as_bytes()[..4]).await.unwrap();
            let _ = response_sent.send(());
            let mut buffer = [0; 1];
            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap()
        });
        let request = tokio::spawn(send_json_gemini::<TestHttpResponse>(
            Client::new().get(format!("http://{address}/generateContent")),
            false,
            0,
            test_http_context(1),
        ));
        response_received.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(server.await.unwrap(), 0);
    }

    #[tokio::test]
    async fn dropping_request_before_response_headers_closes_the_http_stream() {
        use tokio::io::AsyncReadExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_received, request_received_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let _ = request_received.send(());
            let mut buffer = [0; 1];
            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap()
        });
        let request = tokio::spawn(send_json_gemini::<TestHttpResponse>(
            Client::new().get(format!("http://{address}/generateContent")),
            false,
            0,
            test_http_context(1),
        ));
        request_received_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(server.await.unwrap(), 0);
    }

    #[tokio::test]
    async fn cooldown_prevents_requests_until_the_key_becomes_available() {
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
        assert!(result.is_err());
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
    fn key_status_reflects_temporary_cooldowns_without_permanent_rejection() {
        let key = SecretString::from("fake-key-status");
        let now = Instant::now();
        let mut state = KeyRotation::default();
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Available, None, false)
        ));

        state.record_failure(&key, &retry_failure(Some(Duration::from_secs(8))), now);
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::CoolingDown, Some(8), false)
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
            (GeminiApiKeyStatus::CoolingDown, Some(8), false)
        ));
        state.record_success(&key);
        assert!(matches!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Available, None, false)
        ));
    }

    #[test]
    fn daily_quota_inactivates_key_for_the_session_and_rpm_blocks_it_for_sixty_seconds() {
        let daily = SecretString::from("fake-daily-key");
        let rpm = SecretString::from("fake-rpm-key");
        let now = Instant::now();
        let mut state = KeyRotation::default();
        state.record_failure(
            &daily,
            &AttemptFailure {
                action: RetryAction::Retry,
                reason: "Gemini HTTP 429 quota exceeded (requests_per_day)".to_owned(),
                cooldown: Some(Duration::from_secs(30)),
            },
            now,
        );
        state.record_failure(
            &rpm,
            &AttemptFailure {
                action: RetryAction::Retry,
                reason: "Gemini HTTP 429 quota exceeded (requests_per_minute)".to_owned(),
                cooldown: Some(Duration::from_secs(900)),
            },
            now,
        );

        assert_eq!(
            key_status(&state, &daily, now),
            (GeminiApiKeyStatus::InactiveDailyQuota, None, false)
        );
        assert_eq!(
            key_status(&state, &rpm, now),
            (GeminiApiKeyStatus::RpmCooldown, Some(60), false)
        );
        assert!(
            state
                .ordered_keys(std::slice::from_ref(&daily), now)
                .is_err()
        );
        assert!(state.ordered_keys(std::slice::from_ref(&rpm), now).is_err());
        assert_eq!(
            state
                .ordered_keys(std::slice::from_ref(&rpm), now + Duration::from_secs(61))
                .unwrap()
                .len(),
            1
        );
        state.record_success(&daily);
        assert_eq!(
            key_status(&state, &daily, now),
            (GeminiApiKeyStatus::InactiveDailyQuota, None, false)
        );
    }

    #[test]
    fn manual_key_state_is_not_overwritten_by_automatic_failure_and_clear_releases_it() {
        let key = SecretString::from("fake-manual-key");
        let now = Instant::now();
        let mut state = KeyRotation::default();
        state.manual_statuses.push((
            key.clone(),
            GeminiApiKeyManualStatus::InactiveDailyQuota,
            None,
        ));
        state.record_failure(
            &key,
            &AttemptFailure {
                action: RetryAction::Retry,
                reason: "Gemini HTTP 429 quota exceeded (requests_per_minute)".to_owned(),
                cooldown: None,
            },
            now,
        );
        state.record_test_result(&key, GeminiApiKeyTestResult::Verified, now);
        assert_eq!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::InactiveDailyQuota, None, true)
        );
        assert!(state.ordered_keys(std::slice::from_ref(&key), now).is_err());

        state.clear_status(&key);
        assert_eq!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::Available, None, false)
        );
    }

    #[test]
    fn test_probe_classifies_only_explicit_daily_and_rpm_quota_metadata() {
        let response = |metric: &str, quota_id: &str| {
            serde_json::json!({
                "error": {
                    "status": "RESOURCE_EXHAUSTED",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
                        "violations": [{
                            "quotaMetric": metric,
                            "quotaId": quota_id
                        }]
                    }]
                }
            })
            .to_string()
        };
        let daily = classify_http_failure(
            StatusCode::TOO_MANY_REQUESTS,
            None,
            &response("GenerateRequestsPerDayPerProject", "GenerateRequestsPerDay"),
            true,
        );
        let rpm = classify_http_failure(
            StatusCode::TOO_MANY_REQUESTS,
            None,
            &response(
                "GenerateRequestsPerMinutePerProject",
                "GenerateRequestsPerMinute",
            ),
            true,
        );
        let unknown = classify_http_failure(
            StatusCode::TOO_MANY_REQUESTS,
            None,
            r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#,
            true,
        );
        let unavailable = classify_http_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            r#"{"error":{"status":"UNAVAILABLE"}}"#,
            true,
        );
        let rejected = classify_http_failure(
            StatusCode::UNAUTHORIZED,
            None,
            r#"{"error":{"status":"UNAUTHENTICATED"}}"#,
            true,
        );

        assert_eq!(
            test_result_for_failure(&daily),
            GeminiApiKeyTestResult::DailyQuota
        );
        assert_eq!(
            test_result_for_failure(&rpm),
            GeminiApiKeyTestResult::RpmLimited
        );
        assert_eq!(
            test_result_for_failure(&unknown),
            GeminiApiKeyTestResult::Unavailable
        );
        assert_eq!(
            test_result_for_failure(&unavailable),
            GeminiApiKeyTestResult::Unavailable
        );
        assert_eq!(
            test_result_for_failure(&rejected),
            GeminiApiKeyTestResult::Rejected
        );
    }

    #[test]
    fn key_availability_state_is_volatile_and_manual_rpm_expires_after_one_minute() {
        let key = SecretString::from("fake-volatile-rpm-key");
        let now = Instant::now();
        let mut state = KeyRotation::default();
        state.manual_statuses.push((
            key.clone(),
            GeminiApiKeyManualStatus::RpmCooldown,
            Some(now + Duration::from_secs(60)),
        ));
        assert_eq!(
            key_status(&state, &key, now),
            (GeminiApiKeyStatus::RpmCooldown, Some(60), true)
        );
        state.prune_expired(now + Duration::from_secs(61));
        assert_eq!(
            key_status(&state, &key, now + Duration::from_secs(61)),
            (GeminiApiKeyStatus::Available, None, false)
        );

        let restarted = KeyRotation::default();
        assert_eq!(
            key_status(&restarted, &key, now),
            (GeminiApiKeyStatus::Available, None, false)
        );
    }

    #[tokio::test]
    async fn concurrent_requests_keep_the_same_configuration_order() {
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
        assert_eq!(results, ["test-key-a", "test-key-a"]);
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
    fn classifies_structured_quota_dimensions_without_logging_provider_text() {
        let cases = [
            (
                "GenerateRequestsPerMinutePerProjectPerModel",
                "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
                "requests_per_minute",
            ),
            (
                "GenerateContentInputTokensPerMinutePerProjectPerModel",
                "GenerateContentInputTokensPerMinute-FreeTier",
                "tokens_per_minute",
            ),
            (
                "GenerateRequestsPerDayPerProjectPerModel",
                "GenerateRequestsPerDayPerProjectPerModel-FreeTier",
                "requests_per_day",
            ),
        ];
        for (metric, id, expected) in cases {
            let body = serde_json::json!({
                "error": {
                    "status": "RESOURCE_EXHAUSTED",
                    "message": "provider message is not emitted",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.QuotaFailure",
                        "violations": [{
                            "quotaMetric": metric,
                            "quotaId": id,
                            "quotaValue": "123",
                            "quotaLimit": "100",
                            "subject": "project-sensitive-value",
                            "description": "sensitive provider description"
                        }]
                    }]
                }
            });
            let metadata = quota_failure_metadata(&body);
            assert_eq!(metadata.categories, [expected]);
            assert_eq!(metadata.values, [123]);
            assert_eq!(metadata.limits, [100]);
            let failure = classify_http_failure(
                StatusCode::TOO_MANY_REQUESTS,
                None,
                &body.to_string(),
                false,
            );
            assert!(failure.reason.contains(expected));
            assert!(!failure.reason.contains("project-sensitive-value"));
            assert!(!failure.reason.contains("sensitive provider description"));
        }
    }

    #[test]
    fn classifies_429_without_quota_metadata_as_unknown_quota() {
        let failure = classify_http_failure(
            StatusCode::TOO_MANY_REQUESTS,
            None,
            r#"{"error":{"status":"RESOURCE_EXHAUSTED","message":"provider-specific detail"}}"#,
            false,
        );
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(failure.cooldown, None);
        assert_eq!(failure.reason, "Gemini HTTP 429 quota exceeded (unknown)");
        assert!(!failure.reason.contains("provider-specific detail"));
    }

    #[test]
    fn classifies_503_as_project_failover_and_preserves_retry_after_hint() {
        let failure = classify_http_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(Duration::from_secs(9)),
            r#"{"error":{"status":"UNAVAILABLE"}}"#,
            false,
        );
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(failure.cooldown, Some(Duration::from_secs(9)));

        let retry_info_body = serde_json::json!({
            "error": {
                "status": "UNAVAILABLE",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.RetryInfo",
                    "retryDelay": "2.5s"
                }]
            }
        });
        let failure = classify_http_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            &retry_info_body.to_string(),
            false,
        );
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(failure.cooldown, Some(Duration::from_millis(2500)));
    }

    #[test]
    fn classifies_transport_failures_as_failover_candidates() {
        assert_eq!(
            classify_transport_failure(true, false, false).action,
            RetryAction::Retry
        );
        assert_eq!(
            classify_transport_failure(false, true, false).action,
            RetryAction::Retry
        );
        assert_eq!(
            classify_transport_failure(false, true, true).action,
            RetryAction::Retry
        );
        assert_eq!(
            classify_transport_failure(false, false, false).action,
            RetryAction::Retry
        );
    }

    #[test]
    fn server_failures_and_project_permission_errors_advance_to_another_key() {
        let failure = classify_http_failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            None,
            r#"{"error":{"status":"INTERNAL"}}"#,
            false,
        );
        assert_eq!(failure.action, RetryAction::Retry);
        assert_eq!(
            classify_http_failure(
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                r#"{"error":{"status":"UNAVAILABLE"}}"#,
                false
            )
            .action,
            RetryAction::Retry
        );
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
        assert_eq!(
            classify_http_failure(
                StatusCode::FORBIDDEN,
                None,
                r#"{"error":{"status":"PERMISSION_DENIED"}}"#,
                false
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
    fn generic_key_replacement_preserves_order_and_other_keys() {
        let mut records = vec![
            stored_key("primary-id", "old-primary", true),
            stored_key("secondary-id", "secondary-secret", false),
        ];
        records[0].secret = SecretString::from("new-primary");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "primary-id");
        assert_eq!(records[0].secret.expose_secret(), "new-primary");
        assert_eq!(records[1].id, "secondary-id");
        assert_eq!(records[1].secret.expose_secret(), "secondary-secret");
    }

    #[test]
    fn removing_one_key_preserves_other_keys_and_order() {
        let mut records = vec![
            stored_key("primary-id", "primary-secret", true),
            stored_key("secondary-a", "secondary-secret-a", false),
            stored_key("secondary-b", "secondary-secret-b", false),
        ];
        records.remove(1);

        assert_eq!(
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["primary-id", "secondary-b"]
        );
        assert_eq!(records[0].id, "primary-id");
    }

    #[test]
    fn more_than_sixteen_secondary_keys_and_duplicate_keys_are_supported() {
        let mut records = (0..32)
            .map(|index| {
                stored_key(
                    &format!("key-{index}"),
                    &format!("fake-gemini-key-{index}"),
                    index == 0,
                )
            })
            .collect::<Vec<_>>();
        let existing = records[0].secret.clone();
        assert!(add_secondary_record(&mut records, existing).is_err());
        add_secondary_record(&mut records, SecretString::from("fake-new-key")).unwrap();
        assert_eq!(records.len(), 33);
        assert!(normalize_keys(test_keys(&["one", "two"])).is_ok());
    }

    #[test]
    fn key_manifest_stores_only_ids_and_round_trips() {
        let manifest = StoredAdditionalKeyManifest {
            version: KEY_MANIFEST_VERSION,
            initialized: true,
            ids: (0..32).map(|index| format!("test-id-{index}")).collect(),
        };
        let encoded = serde_json::to_string(&manifest).unwrap();
        assert!(!encoded.contains("fake-gemini-key"));
        let decoded: StoredAdditionalKeyManifest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn managed_key_metadata_never_serializes_the_secret_value() {
        let info = GeminiApiKeyInfo {
            id: "opaque-stable-id".to_owned(),
            index: 1,
            status: GeminiApiKeyStatus::Available,
            retry_after_seconds: None,
            manually_set: false,
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

    #[test]
    fn managed_credentials_survive_restart_edit_and_removal_beyond_sixteen_keys() {
        let directory = TemporaryStoreDirectory::create().unwrap();
        let store = FileCredentialStore::new(directory.0.clone());
        let mut records = vec![stored_key("primary", "fake-primary", true)];
        records.extend((0..21).map(|index| {
            stored_key(
                &format!("secondary-{index}"),
                &format!("fake-secondary-{index}"),
                false,
            )
        }));
        persist_key_records_with_store(&store, &records).unwrap();
        assert_eq!(store.load_manifest().unwrap().ids.len(), 22);
        assert!(store.get(ADDITIONAL_KEYS_SECRET).unwrap().is_none());
        let store_root = directory.0.clone();
        drop(store);

        let store = FileCredentialStore::new(store_root);
        let reloaded = load_key_records_with_store(&store).unwrap();
        assert_eq!(reloaded.len(), 22);
        assert_eq!(reloaded[21].secret.expose_secret(), "fake-secondary-20");

        let edit_id = reloaded[4].id.clone();
        let mut edited = reloaded;
        edited[4].secret = SecretString::from("fake-edited-secret");
        persist_key_records_with_store(&store, &edited).unwrap();
        let mut reloaded = load_key_records_with_store(&store).unwrap();
        assert_eq!(
            reloaded
                .iter()
                .find(|record| record.id == edit_id)
                .unwrap()
                .secret
                .expose_secret(),
            "fake-edited-secret"
        );
        reloaded.retain(|record| record.id != edit_id);
        persist_key_records_with_store(&store, &reloaded).unwrap();
        assert!(!store.load_manifest().unwrap().ids.contains(&edit_id));
        assert!(
            store
                .get(&additional_key_secret_name(&edit_id))
                .unwrap()
                .is_none()
        );
        assert_eq!(load_key_records_with_store(&store).unwrap().len(), 21);
    }

    #[tokio::test]
    async fn twelve_unique_keys_load_and_rotate_in_configuration_order() {
        let store = MemoryCredentialStore::default();
        let records = (1..=12)
            .map(|index| {
                stored_key(
                    &format!("opaque-id-{index}"),
                    &format!("simulated-key-{index}"),
                    false,
                )
            })
            .collect::<Vec<_>>();
        persist_key_records_with_store(&store, &records).unwrap();
        let loaded = load_key_records_with_store(&store).unwrap();
        assert_eq!(loaded.len(), 12);
        assert_eq!(
            loaded
                .iter()
                .map(|record| record.secret.expose_secret().to_owned())
                .collect::<Vec<_>>(),
            (1..=12)
                .map(|index| format!("simulated-key-{index}"))
                .collect::<Vec<_>>()
        );

        let calls = std::cell::RefCell::new(Vec::new());
        let result = execute_with_keys(
            &Mutex::new(KeyRotation::default()),
            loaded.into_iter().map(|record| record.secret).collect(),
            |key| {
                calls.borrow_mut().push(key.expose_secret().to_owned());
                async move {
                    if key.expose_secret() == "simulated-key-12" {
                        Ok(key.expose_secret().to_owned())
                    } else {
                        Err(AttemptFailure {
                            action: RetryAction::RejectKey,
                            reason: "simulated authentication failure".to_owned(),
                            cooldown: None,
                        })
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "simulated-key-12");
        assert_eq!(calls.borrow().len(), 12);
        assert_eq!(
            calls.borrow().first().map(String::as_str),
            Some("simulated-key-1")
        );
        assert_eq!(
            calls.borrow().last().map(String::as_str),
            Some("simulated-key-12")
        );
    }

    #[test]
    fn legacy_store_migrates_all_values_into_individual_credentials() {
        let store = MemoryCredentialStore::default();
        store
            .set("gemini", &SecretString::from("fake-primary"))
            .unwrap();
        store
            .set(
                ADDITIONAL_KEYS_SECRET,
                &SecretString::from(r#"["fake-legacy-one","fake-legacy-two"]"#),
            )
            .unwrap();

        let records = load_key_records_with_store(&store).unwrap();

        assert_eq!(
            records
                .iter()
                .map(|record| record.secret.expose_secret())
                .collect::<Vec<_>>(),
            ["fake-primary", "fake-legacy-one", "fake-legacy-two"]
        );
        let manifest = store.load_manifest().unwrap();
        assert!(manifest.initialized);
        assert_eq!(manifest.ids.len(), 3);
        assert!(store.get(ADDITIONAL_KEYS_SECRET).unwrap().is_none());
        for id in manifest.ids {
            assert!(
                store
                    .get(&additional_key_secret_name(&id))
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn credential_store_read_failures_are_reported_and_manifest_write_rolls_back() {
        let store = MemoryCredentialStore::default();
        store.fail_next(StoreOperation::Get);
        assert!(load_key_records_with_store(&store).is_err());

        let original = vec![
            stored_key("primary", "fake-primary", true),
            stored_key("secondary", "fake-secondary", false),
        ];
        persist_key_records_with_store(&store, &original).unwrap();
        let mut updated = original.clone();
        updated[1].secret = SecretString::from("fake-updated");
        store.fail_next(StoreOperation::SaveManifest);
        assert!(persist_key_records_with_store(&store, &updated).is_err());

        let restored = load_key_records_with_store(&store).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[1].secret.expose_secret(), "fake-secondary");
        assert_eq!(
            store.load_manifest().unwrap().ids,
            ["primary".to_owned(), "secondary".to_owned()]
        );
    }
}
