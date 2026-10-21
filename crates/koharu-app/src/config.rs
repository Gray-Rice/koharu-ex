use anyhow::{Context, Result};
use koharu_config::Config;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize, Serialize, Type)]
#[serde(default)]
pub struct ProjectsConfig {
    /// Custom project directory. If not set, defaults to Documents/Koharu.
    pub project_dir: Option<PathBuf>,
}

impl ProjectsConfig {
    pub fn load() -> Result<Config<Self>> {
        koharu_config::load("projects")
    }

    /// Get the effective project directory, falling back to Documents/Koharu if not configured.
    pub fn project_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.project_dir {
            Ok(dir.clone())
        } else {
            let docs = dirs::document_dir().context("the Documents directory is unavailable")?;
            Ok(docs.join("Koharu"))
        }
    }
}
