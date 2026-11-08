use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Type)]
#[serde(default)]
pub struct TypesettingConfig {
    pub font_families: Vec<String>,
}

impl Default for TypesettingConfig {
    fn default() -> Self {
        Self {
            font_families: vec!["CCWildWords".to_owned(), "Adobe 黑体 Std".to_owned()],
        }
    }
}

impl TypesettingConfig {
    pub fn load() -> anyhow::Result<norrix_config::Config<Self>> {
        norrix_config::load("typesetting")
    }
}
