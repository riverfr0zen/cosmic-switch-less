// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, ConfigGet};
use serde::{Deserialize, Serialize};

/// Schema version. Bump alongside `#[serde(default)]`-compatible field
/// additions; bump and add migration logic for breaking changes.
const VERSION: u64 = 1;

/// Single-key storage. All settings live in one file at
/// `~/.config/cosmic/<app-id>/v<VERSION>/settings`.
const KEY: &str = "settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub overlay_width: f32,
    pub list_font_size: Option<f32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            overlay_width: 600.0,
            list_font_size: None,
        }
    }
}

impl Config {
    /// Load the config for `app_id`. Falls back to `Config::default()` if the
    /// config directory/file is missing or the contents fail to parse; real
    /// errors (anything besides a missing file) are logged to stderr.
    pub fn load(app_id: &str) -> Self {
        let helper = match cosmic_config::Config::new(app_id, VERSION) {
            Ok(h) => h,
            Err(why) => {
                eprintln!("cosmic-switch-less: failed to open config helper: {why}");
                return Self::default();
            }
        };
        match helper.get::<Self>(KEY) {
            Ok(config) => config,
            Err(cosmic_config::Error::GetKey(_, err))
                if err.kind() == std::io::ErrorKind::NotFound =>
            {
                // No file yet — user hasn't created one. Expected.
                Self::default()
            }
            Err(why) => {
                eprintln!("cosmic-switch-less: config load error: {why}");
                Self::default()
            }
        }
    }
}
