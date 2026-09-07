use crate::state::{ComponentConfigCache, ComponentProfile, ConfigComponent};
use maestro_core::{
    error::ConfigError,
    soundfont::DEFAULT_SOUNDFONT_LIST_NAME,
    system_cfg::{MaestroConfigManager, MaestroGlobalConfig},
};
use std::path::PathBuf;

pub fn load_global_config() -> MaestroGlobalConfig {
    MaestroConfigManager::default()
        .get_global_config()
        .unwrap_or_default()
}

pub fn save_global_config(config: &MaestroGlobalConfig) -> Result<(), ConfigError> {
    MaestroConfigManager::default().save_global_config(config)
}

pub fn get_soundfont_list_files() -> Vec<(String, PathBuf)> {
    let manager = MaestroConfigManager::default();

    let search_paths = manager.sflist_dirs();

    let mut lists: Vec<(String, PathBuf)> = search_paths
        .into_iter()
        .filter(|p| p.is_dir())
        .filter_map(|p| std::fs::read_dir(p).ok())
        .flat_map(|entries| entries.flatten())
        .map(|entry| entry.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|p| {
            let name = p.file_stem().and_then(|s| s.to_str())?.to_string();
            Some((name, p))
        })
        .collect();

    lists.sort_by(|a, b| a.0.cmp(&b.0));
    lists.dedup_by(|a, b| a.0 == b.0);

    // Make sure the default list always exists on disk
    if !lists
        .iter()
        .any(|(name, _)| name == DEFAULT_SOUNDFONT_LIST_NAME)
    {
        let _ = manager.ensure_default_soundfont_list();
        let path = manager
            .sflists_default_dir()
            .join(format!("{DEFAULT_SOUNDFONT_LIST_NAME}.json"));
        lists.insert(0, (DEFAULT_SOUNDFONT_LIST_NAME.to_string(), path));
    }
    lists
}

const STANDARD_COMPONENTS: [(&str, ConfigComponent); 3] = [
    ("system", ConfigComponent::System),
    ("kdmapi", ConfigComponent::KDMAPI),
    ("converter", ConfigComponent::Converter),
];

pub fn get_component_files() -> Vec<(String, PathBuf)> {
    let manager = MaestroConfigManager::default();
    let mut comps: Vec<(String, PathBuf)> = manager
        .list_components()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| {
            let kind = ConfigComponent::from_name(&name)?;
            let path = manager.get_component_config_path(&kind);
            Some((name, path))
        })
        .collect();

    for (name, std_comp) in STANDARD_COMPONENTS {
        if !comps.iter().any(|c| c.0 == name) {
            comps.push((
                name.to_string(),
                manager.get_component_config_path(&std_comp),
            ));
        }
    }

    comps.sort_by(|a, b| a.0.cmp(&b.0));
    comps
}

fn value_or_default<T: serde::de::DeserializeOwned + Default>(
    val: &serde_json::Value,
    key: &str,
    recovered: &mut bool,
) -> T {
    match val.get(key) {
        Some(v) => serde_json::from_value::<T>(v.clone()).unwrap_or_else(|_| {
            *recovered = true;
            T::default()
        }),
        None => T::default(),
    }
}

pub fn load_config_cache(name: String, path: PathBuf) -> Option<ComponentConfigCache> {
    let raw = path.exists().then(|| std::fs::read_to_string(&path).ok());
    let parsed = raw
        .flatten()
        .map(|content| serde_json::from_str::<serde_json::Value>(&content));

    // Whole-file corruption (present but not parseable) counts as a recovery
    let mut recovered = matches!(parsed, Some(Err(_)));
    let val = match parsed {
        Some(Ok(v)) => v,
        _ => serde_json::Value::Null,
    };

    let enabled = match val.get("enabled") {
        Some(v) => v.as_bool().unwrap_or_else(|| {
            recovered = true;
            true
        }),
        None => true,
    };
    let sflist = match val.get("sflist") {
        Some(v) => v.as_str().map(str::to_string).unwrap_or_else(|| {
            recovered = true;
            DEFAULT_SOUNDFONT_LIST_NAME.to_string()
        }),
        None => DEFAULT_SOUNDFONT_LIST_NAME.to_string(),
    };

    // Event processing is off unless the file explicitly carries a config;
    // post processing defaults on (components default to `Some(..)`), so only
    // an explicit `null` disables it
    let has_event_processor = val.get("event_processor").is_some_and(|v| !v.is_null());
    let has_post_processor = val.get("post_processor").is_none_or(|v| !v.is_null());

    // If the name doesn't match to a component return None (error)
    let profile = ComponentProfile::for_name(&name)?;

    let cache = ComponentConfigCache {
        profile,
        name,
        dirty: false,
        enabled,
        sflist,
        audio_params: value_or_default(&val, "audio_params", &mut recovered),
        realtime: value_or_default(&val, "realtime", &mut recovered),
        renderer: value_or_default(&val, "renderer", &mut recovered),
        has_event_processor,
        event_processor: value_or_default(&val, "event_processor", &mut recovered),
        has_post_processor,
        post_processor: value_or_default(&val, "post_processor", &mut recovered),
        converter_custom: value_or_default(&val, "custom", &mut recovered),
        system_custom: value_or_default(&val, "custom", &mut recovered),
        val,
        path,
    };

    Some(cache)
}

pub fn save_config_cache(cache: &ComponentConfigCache) -> Result<(), Box<dyn std::error::Error>> {
    let mut val = cache.val.clone();
    if !val.is_object() {
        val = serde_json::Value::Object(serde_json::Map::new());
    }
    let obj = val
        .as_object_mut()
        .expect("value was just coerced to an object");
    let p = &cache.profile;

    // Sections present for every component that declares them
    let mut put = |key: &str, value: serde_json::Value| {
        obj.insert(key.to_string(), value);
    };
    put(
        "config_version",
        serde_json::to_value(maestro_core::system_cfg::CONFIG_SCHEMA_VERSION)?,
    );
    if p.has_enabled {
        put("enabled", serde_json::to_value(cache.enabled)?);
    }
    if p.has_sflist {
        put("sflist", serde_json::to_value(&cache.sflist)?);
    }
    if p.has_audio_params {
        put("audio_params", serde_json::to_value(cache.audio_params)?);
    }
    if p.has_realtime {
        put("realtime", serde_json::to_value(&cache.realtime)?);
    }
    if p.has_renderer {
        put("renderer", serde_json::to_value(cache.renderer)?);
    }

    // These two need to be "null" to specify that they are disabled
    if p.has_event_processor {
        put(
            "event_processor",
            match cache.has_event_processor {
                true => serde_json::to_value(&cache.event_processor)?,
                false => serde_json::Value::Null,
            },
        );
    }
    if p.has_post_processor {
        put(
            "post_processor",
            match cache.has_post_processor {
                true => serde_json::to_value(cache.post_processor)?,
                false => serde_json::Value::Null,
            },
        );
    }

    // Component-specific custom payload
    match p.kind {
        ConfigComponent::Converter => put("custom", serde_json::to_value(&cache.converter_custom)?),
        ConfigComponent::System => put("custom", serde_json::to_value(&cache.system_custom)?),
        _ => {}
    }

    if let Some(parent) = cache.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cache.path, serde_json::to_string_pretty(&val)?)?;
    Ok(())
}
