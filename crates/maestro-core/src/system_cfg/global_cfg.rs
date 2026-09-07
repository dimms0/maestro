use std::path::PathBuf;

use crate::soundfont::DEFAULT_SOUNDFONT_LIST_NAME;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MaestroGlobalConfig {
    pub sflist_search_paths: Vec<PathBuf>,
    pub default_soundfont_list: Option<String>,
    pub start_virtual_device_on_launch: bool,
    pub remember_last_tab: bool,
    pub last_tab: String,
}

impl Default for MaestroGlobalConfig {
    fn default() -> Self {
        Self {
            sflist_search_paths: vec![],
            default_soundfont_list: Some(DEFAULT_SOUNDFONT_LIST_NAME.into()),
            start_virtual_device_on_launch: true,
            remember_last_tab: false,
            last_tab: String::new(),
        }
    }
}
