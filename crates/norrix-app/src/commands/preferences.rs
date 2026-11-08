use std::fmt;

use anyhow::Result;
use norrix_pipeline::PipelineConfig;
use norrix_renderer::TypesettingConfig;
use norrix_secrets::ExposeSecret as _;
use norrix_translator::{Language, Model, Provider, ProviderConfig, ProvidersConfig};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::Error;

#[derive(Clone, Debug, Serialize, Type)]
pub struct Preferences {
    pub pipeline: PipelineConfig,
    pub providers: ProviderPreferences,
    pub typesetting: TypesettingConfig,
    pub languages: Vec<LanguageChoice>,
}

impl Preferences {
    pub(crate) fn load() -> Result<Self> {
        let pipeline = PipelineConfig::load()?;
        let providers = ProvidersConfig::load()?;
        let typesetting = TypesettingConfig::load()?;
        let pipeline = pipeline.read()?;
        let providers = providers.read()?;
        let typesetting = typesetting.read()?;
        Ok(Self {
            pipeline: pipeline.clone(),
            providers: ProviderPreferences::from_config(&providers)?,
            typesetting: typesetting.clone(),
            languages: Language::ALL
                .iter()
                .map(|language| LanguageChoice {
                    tag: language.tag().to_owned(),
                    name: language.to_string(),
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Type)]
pub struct ProviderPreferences {
    pub entries: Vec<ProviderPreference>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Type)]
pub struct ProviderPreference {
    pub name: String,
    pub config: ProviderConfig,
    pub credential: Option<CredentialInput>,
}

impl ProviderPreferences {
    fn from_config(config: &ProvidersConfig) -> Result<Self> {
        let entries = config
            .entries()
            .into_iter()
            .map(|config| {
                let provider = config.provider();
                let credential = if provider == Provider::Local {
                    None
                } else {
                    let key: &'static str = provider.into();
                    Some(CredentialInput::load(key)?)
                };
                Ok(ProviderPreference {
                    name: provider.name().to_owned(),
                    config,
                    credential,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { entries })
    }

    fn into_config(self) -> Result<ProvidersConfig> {
        let mut configs = Vec::with_capacity(self.entries.len());
        let mut credentials = Vec::with_capacity(self.entries.len().saturating_sub(1));
        for entry in self.entries {
            let provider = entry.config.provider();
            match entry.credential {
                None if provider == Provider::Local => {}
                Some(credential) if provider != Provider::Local => {
                    let key: &'static str = provider.into();
                    credentials.push((key, credential));
                }
                None => anyhow::bail!("missing credential input for {provider}"),
                Some(_) => anyhow::bail!("local translation does not accept credentials"),
            }
            configs.push(entry.config);
        }
        let config = ProvidersConfig::from_entries(configs)?;
        for (key, credential) in credentials {
            credential.save(key)?;
        }
        Ok(config)
    }
}

#[derive(Clone, Default, Deserialize, Serialize, Type)]
pub struct CredentialInput {
    pub configured: bool,
    pub value: Option<String>,
    pub clear: bool,
}

impl fmt::Debug for CredentialInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialInput")
            .field("configured", &self.configured)
            .field("value", &self.value.as_ref().map(|_| "[REDACTED]"))
            .field("clear", &self.clear)
            .finish()
    }
}

impl CredentialInput {
    fn load(key: &str) -> Result<Self> {
        let configured = if key == "gemini" {
            norrix_translator::gemini_api_keys_configured()?
        } else {
            norrix_secrets::get(key)?
                .is_some_and(|secret| !secret.expose_secret().trim().is_empty())
        };
        Ok(Self {
            configured,
            value: None,
            clear: false,
        })
    }

    fn save(self, key: &str) -> Result<()> {
        if key == "gemini" {
            if self.clear {
                norrix_translator::clear_gemini_api_keys()?;
            } else if let Some(value) = self.value {
                if value.trim().is_empty() {
                    norrix_translator::clear_gemini_api_keys()?;
                } else {
                    norrix_translator::set_gemini_primary_api_key(value.into())?;
                }
            }
            return Ok(());
        }
        if self.clear {
            norrix_secrets::delete(key)?;
        } else if let Some(value) = self.value {
            if value.trim().is_empty() {
                norrix_secrets::delete(key)?;
            } else {
                norrix_secrets::set(key, &value.into())?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct LanguageChoice {
    pub tag: String,
    pub name: String,
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "preferences_saved",
    skip_all,
    fields(setting = "application")
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn save_preferences(
    mut pipeline: PipelineConfig,
    providers: ProviderPreferences,
    typesetting: TypesettingConfig,
) -> std::result::Result<Preferences, Error> {
    remember_pipeline_profiles(&mut pipeline);
    let providers = providers.into_config()?;
    let pipeline_config = PipelineConfig::load()?;
    let providers_config = ProvidersConfig::load()?;
    let typesetting_config = TypesettingConfig::load()?;
    {
        let mut current = pipeline_config.write()?;
        *current = pipeline;
        current.save()?;
    }
    {
        let mut current = providers_config.write()?;
        *current = providers;
        current.save()?;
    }
    {
        let mut current = typesetting_config.write()?;
        *current = typesetting;
        current.save()?;
    }
    let preferences = Preferences::load()?;
    tracing::info!(
        target: "norrix_metrics",
        metric = "preference_changed",
        setting = "application",
    );
    Ok(preferences)
}

fn remember_pipeline_profiles(config: &mut PipelineConfig) {
    let norrix_pipeline::DetectionModel::KoharuLayoutRFDetrSeg2XL(settings) = &config.detection;
    config.processor.koharu_layout_rfdetr_seg_2xl = Some(settings.clone());
    if let norrix_pipeline::InpaintingModel::Flux2Klein(settings) = &config.inpainting {
        config.processor.flux2_klein = Some(settings.clone());
    }
    if let norrix_pipeline::InpaintingModel::RoremMixed(settings) = &config.inpainting {
        config.processor.rorem_mixed = Some(settings.clone());
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_preferences() -> std::result::Result<Preferences, Error> {
    Ok(Preferences::load()?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_translation_models() -> std::result::Result<Vec<Model>, Error> {
    Ok(norrix_translator::Translator::models().await?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_gemini_api_keys()
-> std::result::Result<Vec<norrix_translator::GeminiApiKeyInfo>, Error> {
    Ok(norrix_translator::gemini_api_key_info()?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_gemini_primary_key(
    key: String,
) -> std::result::Result<Vec<norrix_translator::GeminiApiKeyInfo>, Error> {
    Ok(norrix_translator::set_gemini_primary_api_key_with_info(
        key.into(),
    )?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn add_gemini_secondary_key(
    key: String,
) -> std::result::Result<Vec<norrix_translator::GeminiApiKeyInfo>, Error> {
    Ok(norrix_translator::add_gemini_secondary_api_key_with_info(
        key.into(),
    )?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_gemini_secondary_key(
    id: String,
) -> std::result::Result<Vec<norrix_translator::GeminiApiKeyInfo>, Error> {
    Ok(norrix_translator::remove_gemini_secondary_api_key_with_info(&id)?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn clear_gemini_api_keys()
-> std::result::Result<Vec<norrix_translator::GeminiApiKeyInfo>, Error> {
    Ok(norrix_translator::clear_gemini_api_keys_with_info()?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn test_gemini_api_key(
    id: String,
) -> std::result::Result<norrix_translator::GeminiApiKeyTestResult, Error> {
    Ok(norrix_translator::test_gemini_api_key(&id).await?)
}
