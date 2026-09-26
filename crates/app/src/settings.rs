//! Preferences that survive a restart: theme, time axis, mode bands, which
//! panels are open and what the bottom one shows, map tiles and recent
//! files. Stored through eframe's key-value storage, a file next to the
//! app's config on native.

use std::path::{Path, PathBuf};

use egui::ThemePreference;
use serde::{Deserialize, Serialize};

/// Recent paths kept in the File menu.
const MAX_RECENT: usize = 8;

/// What the time axis reads in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeAxis {
    /// Seconds since the autopilot booted, the log's own clock.
    #[default]
    Boot,
    /// UTC through the first GPS fix; boot time when the log has none.
    Utc,
}

/// What the bottom panel lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BottomTab {
    #[default]
    Events,
    Parameters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent toggles, each a setting in its own right"
)]
pub struct Settings {
    /// Dark unless the user asks otherwise: plots read better on a dark
    /// ground, whatever the desktop is set to.
    pub theme: ThemePreference,
    pub time_axis: TimeAxis,
    /// Flight mode bands behind the plot.
    pub show_modes: bool,
    /// The side panel unfolded.
    pub side_panel: bool,
    pub show_map: bool,
    /// The bottom panel, with the events list and the parameter table.
    /// Settings saved before the panel had tabs call it `show_events`.
    #[serde(alias = "show_events")]
    pub show_bottom: bool,
    pub bottom_tab: BottomTab,
    /// Map tiles downloaded from OpenStreetMap; off, the track draws on a
    /// plain background and nothing leaves the machine.
    pub online_tiles: bool,
    /// Most recent first.
    pub recent: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Dark,
            time_axis: TimeAxis::default(),
            show_modes: true,
            side_panel: true,
            show_map: true,
            show_bottom: true,
            bottom_tab: BottomTab::default(),
            online_tiles: true,
            recent: Vec::new(),
        }
    }
}

impl Settings {
    const KEY: &str = "aftermission-settings";

    /// Read saved settings, or the defaults when there are none.
    #[must_use]
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| eframe::get_value(s, Self::KEY))
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::KEY, self);
    }

    /// Put `path` at the front of the recent list.
    pub fn remember(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_ron_and_tolerates_missing_fields() {
        let mut settings = Settings {
            theme: ThemePreference::Light,
            time_axis: TimeAxis::Utc,
            show_modes: false,
            ..Settings::default()
        };
        settings.remember(Path::new("flight.bin"));
        let text = ron::to_string(&settings).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back, settings);

        // a settings file from an older build lacks newer fields
        let partial: Settings = ron::from_str("(side_panel: false)").unwrap();
        assert!(!partial.side_panel);
        assert_eq!(partial.theme, ThemePreference::Dark);
        assert_eq!(partial.time_axis, TimeAxis::Boot);
        assert!(partial.show_modes);
        assert!(partial.show_map && partial.show_bottom && partial.online_tiles);
        assert_eq!(partial.bottom_tab, BottomTab::Events);
        assert!(partial.recent.is_empty());

        // the bottom panel's toggle keeps its saved value under its old name
        let renamed: Settings = ron::from_str("(show_events: false)").unwrap();
        assert!(!renamed.show_bottom);
    }

    #[test]
    fn recent_list_dedupes_and_caps() {
        let mut settings = Settings::default();
        for i in 0..12 {
            settings.remember(Path::new(&format!("{i}.bin")));
        }
        settings.remember(Path::new("3.bin"));
        assert_eq!(settings.recent.len(), MAX_RECENT);
        assert_eq!(settings.recent[0], PathBuf::from("3.bin"));
        assert_eq!(settings.recent[1], PathBuf::from("11.bin"));
        assert_eq!(
            settings
                .recent
                .iter()
                .filter(|p| p.ends_with("3.bin"))
                .count(),
            1
        );
    }
}
