use std::fmt;

use anyhow::{Context, Result};
use koharu_pipeline::PipelineConfig;
use koharu_renderer::TypesettingConfig;
use koharu_secrets::ExposeSecret as _;
use koharu_translator::{Language, Model, Provider, ProviderConfig, ProvidersConfig};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use super::{Error, project::ProjectLibrary};
use crate::config::ProjectsConfig;

#[derive(Clone, Debug, Serialize, Type)]
pub struct Preferences {
    pub pipeline: PipelineConfig,
    pub providers: ProviderPreferences,
    pub typesetting: TypesettingConfig,
    pub languages: Vec<LanguageChoice>,
    pub project_directory: String,
}

impl Preferences {
    pub(crate) fn load() -> Result<Self> {
        let pipeline = PipelineConfig::load()?;
        let providers = ProvidersConfig::load()?;
        let typesetting = TypesettingConfig::load()?;
        let projects = ProjectsConfig::load()?;
        let pipeline = pipeline.read()?;
        let providers = providers.read()?;
        let typesetting = typesetting.read()?;
        let projects = projects.read()?;
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
            project_directory: projects.project_dir()?.to_string_lossy().to_string(),
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
        Ok(Self {
            configured: koharu_secrets::get(key)?
                .is_some_and(|secret| !secret.expose_secret().trim().is_empty()),
            value: None,
            clear: false,
        })
    }

    fn save(self, key: &str) -> Result<()> {
        if self.clear {
            koharu_secrets::delete(key)?;
        } else if let Some(value) = self.value {
            if value.trim().is_empty() {
                koharu_secrets::delete(key)?;
            } else {
                koharu_secrets::set(key, &value.into())?;
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
    target = "koharu_metrics",
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
        target: "koharu_metrics",
        metric = "preference_changed",
        setting = "application",
    );
    Ok(preferences)
}

fn remember_pipeline_profiles(config: &mut PipelineConfig) {
    let koharu_pipeline::DetectionModel::KoharuLayoutRFDetrSeg2XL(settings) = &config.detection;
    config.processor.koharu_layout_rfdetr_seg_2xl = Some(settings.clone());
    if let koharu_pipeline::InpaintingModel::Flux2Klein(settings) = &config.inpainting {
        config.processor.flux2_klein = Some(settings.clone());
    }
    if let koharu_pipeline::InpaintingModel::RoremMixed(settings) = &config.inpainting {
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
    Ok(koharu_translator::Translator::models().await?)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_project_directory() -> std::result::Result<String, Error> {
    let config = ProjectsConfig::load()?;
    let dir = config.read()?.project_dir()?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_project_directory(
    window: tauri::WebviewWindow<tauri_runtime_cef::CefRuntime>,
) -> std::result::Result<Option<String>, Error> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Select Project Directory")
        .set_parent(&window);
    let Some(folder) = dialog.pick_folder().await else {
        return Ok(None);
    };
    Ok(Some(folder.path().to_string_lossy().to_string()))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_project_directory(
    path: String,
    library: State<'_, ProjectLibrary>,
) -> std::result::Result<String, Error> {
    let path_buf = std::path::PathBuf::from(&path);

    // Validate the path
    if !path_buf.is_absolute() {
        return Err(Error::from(anyhow::anyhow!("project directory must be an absolute path")));
    }

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&path_buf)
        .with_context(|| format!("failed to create project directory {}", path_buf.display()))?;

    // Check if directory is writable
    let test_file = path_buf.join(".koharu_write_test");
    std::fs::write(&test_file, b"test")
        .with_context(|| format!("project directory {} is not writable", path_buf.display()))?;
    std::fs::remove_file(&test_file).ok();

    // Save to config
    let config = ProjectsConfig::load()?;
    {
        let mut current = config.write()?;
        current.project_dir = Some(path_buf.clone());
        current.save()?;
    }

    // Re-point the in-memory project library at the new root so the project
    // list reflects the change without an application restart.
    library.refresh()?;

    Ok(path_buf.to_string_lossy().to_string())
}
