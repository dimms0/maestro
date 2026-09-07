use std::path::PathBuf;

use crate::paths::{self, LibSource};

use super::bassmidi::library::{
    BASS_LIB_FILENAME, BASSFLAC_LIB_FILENAME, BASSMIDI_LIB_FILENAME, BASSMIDISharedLib,
    BASSSharedLib,
};
use super::fluidsynth::library::{FLUIDSYNTH_LIB_CANDIDATES, FluidSynthSharedLib};

pub struct LibraryStatus {
    pub name: &'static str,
    pub filename: String,
    pub source: Option<LibSource>,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub error: Option<String>,
}

impl LibraryStatus {
    fn missing(name: &'static str, filename: &str, error: String) -> Self {
        let error = match paths::mismatched_lib(filename) {
            Some((path, arch)) => format!(
                "{} was built for {arch}; Maestro is running as {}.",
                path.display(),
                std::env::consts::ARCH
            ),
            None => error,
        };
        Self {
            name,
            filename: filename.to_string(),
            source: None,
            path: None,
            version: None,
            error: Some(error),
        }
    }

    fn found(
        name: &'static str,
        filename: &str,
        path: Option<PathBuf>,
        version: Option<String>,
    ) -> Self {
        Self {
            name,
            filename: filename.to_string(),
            source: Some(path.as_deref().map_or(LibSource::System, paths::lib_source)),
            path,
            version,
            error: None,
        }
    }
}

fn packed_version(packed: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        packed >> 24,
        (packed >> 16) & 0xFF,
        (packed >> 8) & 0xFF,
        packed & 0xFF
    )
}

pub fn probe_libraries() -> Vec<LibraryStatus> {
    // BASSMIDI and BASSFLAC link against BASS, so it has to stay loaded while
    // they are probed or they resolve nothing and look missing.
    let bass = unsafe { BASSSharedLib::new(BASS_LIB_FILENAME) };

    let bass_status = match &bass {
        Ok(lib) => {
            let version = packed_version(unsafe { (lib.BASS_GetVersion)() });
            LibraryStatus::found("BASS", BASS_LIB_FILENAME, lib.path.clone(), Some(version))
        }
        Err(e) => LibraryStatus::missing("BASS", BASS_LIB_FILENAME, e.to_string()),
    };

    let statuses = vec![
        bass_status,
        probe_bassmidi(),
        probe_bassflac(),
        probe_fluidsynth(),
    ];

    drop(bass);
    statuses
}

fn probe_bassmidi() -> LibraryStatus {
    match unsafe { BASSMIDISharedLib::new(BASSMIDI_LIB_FILENAME) } {
        Ok(lib) => {
            let version = packed_version(unsafe { (lib.BASS_MIDI_GetVersion)() });
            LibraryStatus::found(
                "BASSMIDI",
                BASSMIDI_LIB_FILENAME,
                lib.path.clone(),
                Some(version),
            )
        }
        Err(e) => LibraryStatus::missing("BASSMIDI", BASSMIDI_LIB_FILENAME, e.to_string()),
    }
}

fn probe_bassflac() -> LibraryStatus {
    match paths::load_library(BASSFLAC_LIB_FILENAME) {
        Ok((_, path)) => LibraryStatus::found("BASSFLAC", BASSFLAC_LIB_FILENAME, path, None),
        Err(e) => LibraryStatus::missing("BASSFLAC", BASSFLAC_LIB_FILENAME, e.to_string()),
    }
}

fn probe_fluidsynth() -> LibraryStatus {
    let mut last_err = None;

    for candidate in FLUIDSYNTH_LIB_CANDIDATES {
        match unsafe { FluidSynthSharedLib::new(candidate) } {
            Ok(lib) => {
                let (mut major, mut minor, mut micro) = (0, 0, 0);
                unsafe { (lib.fluid_version)(&mut major, &mut minor, &mut micro) };
                return LibraryStatus::found(
                    "FluidSynth",
                    candidate,
                    lib.path.clone(),
                    Some(format!("{major}.{minor}.{micro}")),
                );
            }
            Err(e) => last_err = Some(e),
        }
    }

    LibraryStatus::missing(
        "FluidSynth",
        FLUIDSYNTH_LIB_CANDIDATES[0],
        last_err.map(|e| e.to_string()).unwrap_or_default(),
    )
}
