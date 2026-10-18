mod dirs;
mod global_cfg;
pub mod system;
#[cfg(feature = "config-watcher")]
pub mod watcher;

pub use global_cfg::*;

use crate::soundfont::SoundFontList;
use crate::{error::ConfigError, soundfont::DEFAULT_SOUNDFONT_LIST_NAME};
use dirs::ConfigDirs;
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigComponent {
    System,
    KDMAPI,
    Converter,
    Api,
}

impl ConfigComponent {
    pub fn to_filename(&self) -> String {
        match self {
            Self::System => "system".to_string(),
            Self::KDMAPI => "kdmapi".to_string(),
            Self::Converter => "converter".to_string(),
            Self::Api => "api".to_string(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "system" => Some(Self::System),
            "kdmapi" => Some(Self::KDMAPI),
            "converter" => Some(Self::Converter),
            "api" => Some(Self::Api),
            _ => None,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::System => "Virtual Device".to_string(),
            Self::KDMAPI => "KDMAPI".to_string(),
            Self::Converter => "Converter".to_string(),
            Self::Api => "Maestro API".to_string(),
        }
    }

    /// Stable wire encoding for the statistics frame, which has to say which
    /// component a publisher belongs to so the GUI can tell the daemon's frame
    /// apart from an in-process one.
    pub fn code(&self) -> u32 {
        match self {
            Self::System => 0,
            Self::KDMAPI => 1,
            Self::Converter => 2,
            Self::Api => 3,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::System),
            1 => Some(Self::KDMAPI),
            2 => Some(Self::Converter),
            3 => Some(Self::Api),
            _ => None,
        }
    }
}

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

pub const APP_PROFILE_DIR: &str = "apps";

pub fn is_valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

pub struct MaestroConfigManager {
    dirs: ConfigDirs,
}

impl Default for MaestroConfigManager {
    fn default() -> Self {
        Self {
            dirs: ConfigDirs::new(),
        }
    }
}

impl MaestroConfigManager {
    pub fn get_component_config_path(&self, component: &ConfigComponent) -> PathBuf {
        self.dirs
            .config_components()
            .join(format!("{}.json", component.to_filename()))
    }

    pub fn get_app_config_path(&self, app_id: &str) -> PathBuf {
        self.dirs
            .config_components()
            .join(APP_PROFILE_DIR)
            .join(format!("{}.json", app_id.to_lowercase()))
    }

    pub fn sflists_default_dir(&self) -> PathBuf {
        self.dirs.sflists()
    }

    pub fn sflist_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = match self.get_global_config() {
            Ok(global) => global.sflist_search_paths,
            Err(_) => Vec::new(),
        };
        dirs.push(self.sflists_default_dir());
        dirs
    }

    pub fn get_global_config(&self) -> Result<MaestroGlobalConfig, ConfigError> {
        let path = self.dirs.config_root().join("global_config.json");
        self.load_or_default(&path)
    }

    pub fn save_global_config(&self, config: &MaestroGlobalConfig) -> Result<(), ConfigError> {
        let path = self.dirs.config_root().join("global_config.json");
        self.save_json(&path, config)
    }

    pub fn get_soundfont_list(&self, name: &str) -> Result<SoundFontList, ConfigError> {
        self.load_soundfont_list(name).map(|(list, _)| list)
    }

    pub fn load_soundfont_list(
        &self,
        name: &str,
    ) -> Result<(SoundFontList, Vec<PathBuf>), ConfigError> {
        for path in &self.sflist_dirs() {
            let full_path = path.join(format!("{}.json", name));
            if full_path.exists() {
                let mut list: SoundFontList = self.load_json(&full_path)?;
                let removed = list.remove_missing();
                return Ok((list, removed));
            }
        }

        // If not found, create it in the primary sflist directory so a
        // requested list is always present on disk after this call.
        let default_path = self.dirs.sflists().join(format!("{}.json", name));
        let default_list = match crate::paths::bundled_soundfont() {
            // Only a new Default list gets the bundled SoundFont
            Some(font) if name == DEFAULT_SOUNDFONT_LIST_NAME => SoundFontList::from(font),
            _ => SoundFontList::new_with_default_sublist(),
        };
        self.save_json(&default_path, &default_list)?;

        Ok((default_list, Vec::new()))
    }

    pub fn ensure_default_soundfont_list(&self) -> Result<SoundFontList, ConfigError> {
        self.get_soundfont_list(DEFAULT_SOUNDFONT_LIST_NAME)
    }

    pub fn load_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &PathBuf,
    ) -> Result<T, ConfigError> {
        let content = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save_json<T: serde::Serialize>(
        &self,
        path: &PathBuf,
        config: &T,
    ) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut value = serde_json::to_value(config)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("config_version".into(), CONFIG_SCHEMA_VERSION.into());
        }
        fs::write(path, serde_json::to_string_pretty(&value)?)?;
        Ok(())
    }

    pub fn load_or_default<T: serde::de::DeserializeOwned + serde::Serialize + Default>(
        &self,
        path: &PathBuf,
    ) -> Result<T, ConfigError> {
        if !path.exists() {
            let config = T::default();
            self.save_json(path, &config)?;
            return Ok(config);
        }

        match self.load_json(path) {
            Ok(config) => Ok(config),
            Err(ConfigError::Io(..)) | Err(ConfigError::NoConfigDir) => {
                // If it doesn't exist, create the file and the folders
                let config = T::default();
                self.save_json(path, &config)?;
                Ok(config)
            }
            Err(err) => Err(err),
        }
    }

    pub fn list_components(&self) -> Result<Vec<String>, ConfigError> {
        self.list_json_in_dir(self.dirs.config_components())
    }

    fn list_json_in_dir(&self, path: PathBuf) -> Result<Vec<String>, ConfigError> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let mut names = Vec::new();
        self.collect_json_files(&path, &path, &mut names)?;
        Ok(names)
    }

    fn collect_json_files(
        &self,
        root: &PathBuf,
        dir: &PathBuf,
        names: &mut Vec<String>,
    ) -> Result<(), ConfigError> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.collect_json_files(root, &path, names)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("json")
                && let Ok(rel_path) = path.strip_prefix(root)
            {
                // I hate Windows
                let name = rel_path
                    .with_extension("")
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/");
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
        Ok(())
    }
}
