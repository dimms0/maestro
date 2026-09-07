use std::{env, path::PathBuf};

use directories::ProjectDirs;

pub(super) struct ConfigDirs {
    dirs: Option<ProjectDirs>,
}

impl ConfigDirs {
    pub fn new() -> Self {
        Self {
            dirs: ProjectDirs::from("gr", "dimms", "maestro"),
        }
    }

    fn fallback() -> PathBuf {
        if let Ok(cwd) = env::current_dir() {
            return cwd;
        }

        if let Ok(exe_path) = env::current_exe()
            && let Some(exe_dir) = exe_path.parent()
        {
            return exe_dir.to_path_buf();
        }

        PathBuf::from("/")
    }

    pub fn config_root(&self) -> PathBuf {
        if let Some(dirs) = &self.dirs {
            dirs.config_dir().into()
        } else {
            Self::fallback()
        }
    }

    pub fn config_components(&self) -> PathBuf {
        self.config_root().join("components")
    }

    pub fn sflists(&self) -> PathBuf {
        if let Some(dirs) = &self.dirs {
            PathBuf::from(dirs.data_local_dir())
        } else {
            Self::fallback()
        }
        .join("soundfont-lists")
    }
}
