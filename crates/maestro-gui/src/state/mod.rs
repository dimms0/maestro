mod component;

pub use component::{ComponentProfile, ConfigComponent, ConverterCustom, SystemCustomSettings};

use maestro_core::{
    audio_params::AudioParameters,
    realtime::config::RealtimeConfig,
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    soundfont::{DEFAULT_SOUNDFONT_LIST_NAME, SoundFontList},
};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct SoundfontFileCache {
    pub path: PathBuf,
    pub name: String,
    pub dirty: bool,
    pub list: SoundFontList,
}

impl SoundfontFileCache {
    pub fn sorted_sublists(&self) -> Vec<String> {
        let mut sublists: Vec<String> = self.list.lists.keys().cloned().collect();
        sublists.sort();
        if sublists.is_empty() {
            sublists.insert(0, DEFAULT_SOUNDFONT_LIST_NAME.to_string());
        }
        sublists
    }

    pub fn sublist_name(&self, index: i32) -> Option<String> {
        let sublists = self.sorted_sublists();
        usize::try_from(index)
            .ok()
            .and_then(|i| sublists.get(i).cloned())
    }
}

#[derive(Clone, Debug)]
pub struct ComponentConfigCache {
    pub path: PathBuf,
    pub name: String,
    pub dirty: bool,
    pub profile: ComponentProfile,
    /// Raw JSON preserved so unknown/third-party keys survive a save round-trip
    pub val: serde_json::Value,

    pub enabled: bool,
    pub sflist: String,
    pub audio_params: AudioParameters,
    pub realtime: RealtimeConfig,
    pub renderer: RendererConfig,

    pub has_event_processor: bool,
    pub event_processor: EventProcessorConfig,
    pub has_post_processor: bool,
    pub post_processor: PostProcessorConfig,

    pub converter_custom: ConverterCustom,
    pub system_custom: SystemCustomSettings,
}

pub struct AppData {
    pub sflist_files: Vec<SoundfontFileCache>,
    pub selected_sflist_idx: i32,

    pub config_files: Vec<ComponentConfigCache>,
    pub selected_config_idx: i32,

    pub render_queue: Vec<crate::SlintMidiRenderEntry>,
    pub available_sflists: Vec<String>,

    pub audio: AudioDeviceCache,

    pub standalone_mode: crate::StandaloneMode,
}

#[derive(Clone, Debug, Default)]
pub struct AudioDeviceCache {
    pub host_ids: Vec<String>,
    pub host_labels: Vec<String>,
    pub device_ids: Vec<String>,
    pub device_labels: Vec<String>,
    pub sample_rates: Vec<u32>,
    pub buffer_range: Option<(u32, u32)>,
}

impl AudioDeviceCache {
    fn index_of(ids: &[String], id: Option<&str>) -> i32 {
        let Some(id) = id else {
            return 0;
        };

        ids.iter()
            .position(|candidate| candidate == id)
            .map_or(0, |i| i as i32 + 1)
    }

    pub fn host_index(&self, id: Option<&str>) -> i32 {
        Self::index_of(&self.host_ids, id)
    }

    pub fn device_index(&self, id: Option<&str>) -> i32 {
        Self::index_of(&self.device_ids, id)
    }

    pub fn host_id(&self, index: i32) -> Option<String> {
        usize::try_from(index - 1)
            .ok()
            .and_then(|i| self.host_ids.get(i).cloned())
    }

    pub fn device_id(&self, index: i32) -> Option<String> {
        usize::try_from(index - 1)
            .ok()
            .and_then(|i| self.device_ids.get(i).cloned())
    }

    pub fn fixed_by_host(&self, host_id: Option<&str>) -> bool {
        host_id.is_some_and(|id| id.eq_ignore_ascii_case("jack"))
    }

    pub fn refresh(&mut self, host_id: Option<&str>, device_id: Option<&str>) {
        let hosts = maestro_core::realtime::output_hosts();
        self.host_ids = hosts.iter().map(|(id, _)| id.clone()).collect();
        self.host_labels = hosts.into_iter().map(|(_, name)| name).collect();

        let devices = maestro_core::realtime::output_devices(host_id);
        self.device_ids = devices.iter().map(|(id, _)| id.clone()).collect();
        self.device_labels = devices.into_iter().map(|(_, name)| name).collect();

        // Only the selected device is probed: querying capabilities opens the
        // device, and on ALSA a failed open can leak descriptors and poison the
        // backend for the rest of the process's life.
        let (rates, buffer_range) =
            maestro_core::realtime::device_caps(host_id, device_id).unwrap_or_default();

        self.sample_rates = rates;
        self.buffer_range = buffer_range;
    }
}

impl AppData {
    // SoundFont lists

    pub fn selected_sflist(&self) -> Option<&SoundfontFileCache> {
        index_of(self.selected_sflist_idx, self.sflist_files.len()).map(|i| &self.sflist_files[i])
    }

    pub fn selected_sflist_mut(&mut self) -> Option<&mut SoundfontFileCache> {
        index_of(self.selected_sflist_idx, self.sflist_files.len())
            .map(|i| &mut self.sflist_files[i])
    }

    pub fn sflist_position(&self, name: &str) -> Option<usize> {
        self.sflist_files.iter().position(|f| f.name == name)
    }

    // Used to drive the sflist ComboBoxes by `current-index`
    pub fn available_sflist_index(&self, name: &str) -> i32 {
        self.available_sflists
            .iter()
            .position(|n| n == name)
            .unwrap_or(0) as i32
    }

    pub fn refresh_available_sflists(&mut self) {
        self.available_sflists = self.sflist_files.iter().map(|f| f.name.clone()).collect();
    }

    // Component configs

    pub fn config_position(&self, kind: ConfigComponent) -> Option<usize> {
        self.config_files
            .iter()
            .position(|c| c.profile.kind == kind)
    }

    pub fn config(&self, kind: ConfigComponent) -> Option<&ComponentConfigCache> {
        self.config_position(kind).map(|i| &self.config_files[i])
    }

    pub fn selected_config(&self) -> Option<&ComponentConfigCache> {
        index_of(self.selected_config_idx, self.config_files.len()).map(|i| &self.config_files[i])
    }
}

pub fn index_of(idx: i32, len: usize) -> Option<usize> {
    usize::try_from(idx).ok().filter(|&i| i < len)
}
