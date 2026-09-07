use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SoundFontType {
    Sf2,
    Sf3,
    Sfz,
    Unknown,
}

impl SoundFontType {
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("sf2") => Self::Sf2,
            Some("sf3") => Self::Sf3,
            Some("sfz") => Self::Sfz,
            _ => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Sf2 => "SF2",
            Self::Sf3 => "SF3",
            Self::Sfz => "SFZ",
            Self::Unknown => "?",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SoundFont {
    pub enabled: bool,
    pub path: PathBuf,
    pub bank: i16,
    pub preset: i16,
}

impl SoundFont {
    pub fn font_type(&self) -> SoundFontType {
        SoundFontType::from_path(&self.path)
    }
}

impl Default for SoundFont {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::new(),
            bank: 0,
            preset: -1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SoundFontList {
    pub lists: HashMap<String, Vec<SoundFont>>,
    pub port_assignments: HashMap<usize, String>,
    pub default_list: Option<String>,
}

pub const DEFAULT_SOUNDFONT_LIST_NAME: &str = "Default";

impl From<PathBuf> for SoundFontList {
    fn from(value: PathBuf) -> Self {
        let sf = SoundFont {
            path: value,
            ..Default::default()
        };

        let mut lists = HashMap::new();
        lists.insert(DEFAULT_SOUNDFONT_LIST_NAME.to_string(), vec![sf]);

        Self {
            lists,
            port_assignments: HashMap::new(),
            default_list: Some(DEFAULT_SOUNDFONT_LIST_NAME.to_string()),
        }
    }
}

impl SoundFontList {
    pub fn new_with_default_sublist() -> Self {
        let mut lists = HashMap::new();
        lists.insert(DEFAULT_SOUNDFONT_LIST_NAME.to_string(), Vec::new());

        Self {
            lists,
            port_assignments: HashMap::new(),
            default_list: Some(DEFAULT_SOUNDFONT_LIST_NAME.to_string()),
        }
    }
}
