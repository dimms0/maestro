use maestro_core::{
    paths::{self, LibSource},
    renderer::{LibraryStatus, probe_libraries},
};

use crate::{IntegrationStatus, LibraryAction, SlintLibraryItem};

fn vendor_url(name: &str) -> &'static str {
    match name {
        "FluidSynth" => "https://www.fluidsynth.org/",
        _ => "https://www.un4seen.com/",
    }
}

fn source_label(source: LibSource) -> &'static str {
    match source {
        LibSource::User => "user folder",
        LibSource::Program => "program folder",
        LibSource::System => "system path",
    }
}

fn to_slint(status: &LibraryStatus) -> SlintLibraryItem {
    SlintLibraryItem {
        name: status.name.into(),
        filename: status.filename.as_str().into(),
        version: status.version.clone().unwrap_or_default().into(),
        source: status.source.map(source_label).unwrap_or_default().into(),
        detail: status.error.clone().unwrap_or_default().into(),
        status: match status.source {
            Some(_) => IntegrationStatus::Ok,
            None => IntegrationStatus::Missing,
        },
    }
}

pub fn probe() -> Vec<SlintLibraryItem> {
    probe_libraries().iter().map(to_slint).collect()
}

pub fn act(index: i32, action: LibraryAction) -> Result<(), String> {
    if let LibraryAction::Reveal = action {
        let dir = paths::user_lib_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        return super::open_path(&dir.to_string_lossy());
    }

    let libraries = probe_libraries();
    let status = usize::try_from(index)
        .ok()
        .and_then(|i| libraries.get(i))
        .ok_or_else(|| "That library is no longer listed.".to_string())?;

    super::open_path(vendor_url(status.name))
}
