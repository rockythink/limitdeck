use std::{
    env, fs,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    app::{App, ModelTimeRange},
    locale::Language,
    theme::Theme,
};

const FORMAT_VERSION: u8 = 1;
const MAX_INPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct PreferencesFile {
    version: u8,
    theme: Theme,
    language: Language,
    secondary_limits_visible: bool,
    model_time_range: ModelTimeRange,
}

impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            theme: Theme::default(),
            language: Language::detect(),
            secondary_limits_visible: false,
            model_time_range: ModelTimeRange::default(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Preferences {
    path: Option<PathBuf>,
    values: PreferencesFile,
}

impl Preferences {
    pub(crate) fn load_default() -> Self {
        let Some(path) = default_path() else {
            return Self {
                path: None,
                values: PreferencesFile::default(),
            };
        };
        let values = read_preferences(&path).unwrap_or_default();
        Self {
            path: Some(path),
            values,
        }
    }

    pub(crate) fn apply(&self, app: &mut App) {
        app.apply_preferences(
            self.values.theme,
            self.values.language,
            self.values.secondary_limits_visible,
            self.values.model_time_range,
        );
    }

    pub(crate) fn save(&self, app: &App) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "preferences path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        let encoded = serde_json::to_vec_pretty(&PreferencesFile {
            version: FORMAT_VERSION,
            theme: app.theme(),
            language: app.language(),
            secondary_limits_visible: app.secondary_limits_visible(),
            model_time_range: app.model_time_range(),
        })?;
        if encoded.len() > MAX_INPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "preferences exceed size limit",
            ));
        }
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, encoded)?;
        fs::rename(&temporary, path).inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
    }
}

fn read_preferences(path: &Path) -> io::Result<PreferencesFile> {
    if !path.is_file() {
        return Ok(PreferencesFile::default());
    }
    let mut encoded = Vec::new();
    File::open(path)?
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "preferences exceed size limit",
        ));
    }
    let preferences: PreferencesFile = serde_json::from_slice(&encoded)?;
    if preferences.version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported preferences version",
        ));
    }
    Ok(preferences)
}

fn default_path() -> Option<PathBuf> {
    if let Some(directory) = env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(directory).join("limitdeck/config.json"));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/limitdeck/config.json"))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_path() -> PathBuf {
        env::temp_dir().join(format!(
            "limitdeck-preferences-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn preferences_round_trip_the_observable_interface_state() {
        let path = temporary_path();
        let preferences = Preferences {
            path: Some(path.clone()),
            values: PreferencesFile::default(),
        };
        let mut source = App::new([]);
        source.apply_preferences(
            Theme::Mono,
            Language::Chinese,
            true,
            ModelTimeRange::Hours24,
        );
        preferences.save(&source).unwrap();

        let loaded = Preferences {
            path: Some(path.clone()),
            values: read_preferences(&path).unwrap(),
        };
        let mut restored = App::new([]);
        loaded.apply(&mut restored);

        assert_eq!(restored.theme(), Theme::Mono);
        assert_eq!(restored.language(), Language::Chinese);
        assert!(restored.secondary_limits_visible());
        assert_eq!(restored.model_time_range(), ModelTimeRange::Hours24);
        fs::remove_file(path).unwrap();
    }
}
