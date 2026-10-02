//! What the app keeps between runs, in `~/.signalbox`: the window as it was left, and preferences.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the window looked when last used, so it opens the same way (`~/.signalbox/window.json`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SavedWindow {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub sidebar: bool,
    /// As dragged; `None` for the usual width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f32>,
    pub bounds: Option<SavedBounds>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedTab {
    Start,
    Tmux { session: String, window: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub state: WindowState,
}

/// `window.json` as written, with its tabs not yet read.
#[derive(Deserialize)]
struct WindowFile {
    tabs: Vec<Value>,
    active: usize,
    sidebar: bool,
    #[serde(default)]
    sidebar_width: Option<f32>,
    bounds: Option<SavedBounds>,
}

pub fn load_window_from(path: &Path) -> Option<SavedWindow> {
    let file: WindowFile = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut window = SavedWindow {
        tabs: Vec::new(),
        active: 0,
        sidebar: file.sidebar,
        sidebar_width: file.sidebar_width,
        bounds: file.bounds,
    };
    // A kind of tab this version doesn't open (the folder tabs of earlier versions) is left out.
    for (ix, tab) in file.tabs.into_iter().enumerate() {
        if let Ok(tab) = serde_json::from_value(tab) {
            if ix == file.active {
                window.active = window.tabs.len();
            }
            window.tabs.push(tab);
        }
    }
    Some(window)
}

pub fn save_window_to(path: &Path, window: &SavedWindow) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(window)?)?;
    std::fs::rename(temp, path)
}

pub fn load_window() -> Option<SavedWindow> {
    load_window_from(&crate::sys::data_dir().join("window.json"))
}

pub fn save_window(window: &SavedWindow) -> std::io::Result<()> {
    // Test copies and tests leave the window in daily use alone.
    if crate::sys::dev_mode() || cfg!(test) {
        return Ok(());
    }
    save_window_to(&crate::sys::data_dir().join("window.json"), window)
}

/// App preferences (`~/.signalbox/settings.json`); missing fields keep their defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The terminal font size, when changed from the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(crate::sys::data_dir().join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    if crate::sys::dev_mode() || cfg!(test) {
        return Ok(());
    }
    let path = crate::sys::data_dir().join("settings.json");
    std::fs::create_dir_all(crate::sys::data_dir())?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(settings)?)?;
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("signalbox-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn saved_window_round_trips() {
        let path = temp_file("window");
        let window = SavedWindow {
            tabs: vec![
                SavedTab::Tmux {
                    session: "web".into(),
                    window: Some("@75".into()),
                },
                SavedTab::Start,
            ],
            active: 1,
            sidebar: false,
            sidebar_width: Some(300.),
            bounds: Some(SavedBounds {
                x: 10.,
                y: 20.,
                width: 1180.,
                height: 800.,
                state: WindowState::Maximized,
            }),
        };
        save_window_to(&path, &window).unwrap();
        assert_eq!(load_window_from(&path), Some(window));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tabs_of_a_kind_it_doesnt_open_are_left_out() {
        let path = temp_file("old-window");
        let old = r#"{"tabs": [{"folder": {"path": "/code/app"}}, {"tmux": {"session": "web", "window": null}}, "start"],
            "active": 1, "sidebar": true, "bounds": null}"#;
        std::fs::write(&path, old).unwrap();
        let window = load_window_from(&path).unwrap();
        assert_eq!(
            window.tabs,
            vec![
                SavedTab::Tmux {
                    session: "web".into(),
                    window: None
                },
                SavedTab::Start
            ]
        );
        assert_eq!(window.active, 0, "still the tmux tab");
        std::fs::remove_file(&path).ok();
    }
}
