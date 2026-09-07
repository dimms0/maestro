use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

pub use notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigWatchEvent {
    ConfigChanged,
    SoundfontsChanged,
}

pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    pub fn spawn(
        config_path: PathBuf,
        sflist_dirs: Vec<PathBuf>,
        on_event: impl Fn(ConfigWatchEvent) + Send + 'static,
        on_warn: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, notify::Error> {
        let config_file = config_path.clone();
        let sflist_dirs_cln = sflist_dirs.clone();
        let on_warn = Arc::new(on_warn);
        let on_warn_cb = on_warn.clone();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let event = match res {
                Ok(e) => e,
                Err(err) => {
                    (*on_warn_cb)(format!("File watcher error: {err}"));
                    return;
                }
            };

            if !(event.kind.is_create() || event.kind.is_modify() || event.kind.is_remove()) {
                return;
            }

            for path in &event.paths {
                if let Some(msg) = classify(path, &config_file, &sflist_dirs_cln) {
                    on_event(msg);
                    return;
                }
            }
        })?;

        if let Some(dir) = config_path.parent() {
            watch_dir(&mut watcher, dir, &*on_warn);
        }
        for dir in &sflist_dirs {
            watch_dir(&mut watcher, dir, &*on_warn);
        }

        Ok(Self { _watcher: watcher })
    }
}

fn watch_dir(watcher: &mut RecommendedWatcher, dir: &Path, on_warn: &dyn Fn(String)) {
    // Missing directories aren't fatal, the config manager creates them on
    // first save, and the watch is re-established the next time the consumer
    // starts.
    if let Err(err) = watcher.watch(dir, RecursiveMode::NonRecursive) {
        on_warn(format!("Cannot watch {}: {err}", dir.display()));
    }
}

fn classify(path: &Path, config_file: &Path, sflist_dirs: &[PathBuf]) -> Option<ConfigWatchEvent> {
    if path == config_file {
        return Some(ConfigWatchEvent::ConfigChanged);
    }

    // Any JSON file in the watched soundfont-list directories may be (or
    // become) the active list; the consumer decides whether the active list
    // actually changed. The config components directory holds other
    // components' JSON files, hence the directory check.
    if path.extension().and_then(|e| e.to_str()) == Some("json")
        && path
            .parent()
            .is_some_and(|dir| sflist_dirs.iter().any(|d| d == dir))
    {
        return Some(ConfigWatchEvent::SoundfontsChanged);
    }

    None
}
