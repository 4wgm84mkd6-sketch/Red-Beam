//! User preferences, recent and pinned files. Stored as JSON in
//! %LOCALAPPDATA%\Red-Beam\settings.json. Pure logic (testable anywhere).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::markup::MKind;
use crate::takeoff::{EndStyle, Kind, Units};

/// What a preset draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetTool {
    Measure(Kind),
    Volume,
    Markup(MKind),
}

/// A saved tool with its settings (Bluebeam's Tool Chest).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub tool: PresetTool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub layer: String,
    #[serde(default)]
    pub ends: EndStyle,
    #[serde(default)]
    pub leader: EndStyle,
    #[serde(default)]
    pub depth_in: Option<f64>,
    #[serde(default)]
    pub height_in: Option<f64>,
    #[serde(default)]
    pub slope: Option<f64>,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub font: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Background {
    #[default]
    Dark,
    Gray,
    Light,
}

impl Background {
    pub fn color(self) -> u32 {
        match self {
            Background::Dark => 0x3b3e43,
            Background::Gray => 0x7a7d82,
            Background::Light => 0xd5d7da,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Background::Dark => "Dark",
            Background::Gray => "Gray",
            Background::Light => "Light",
        }
    }
    pub fn next(self) -> Background {
        match self {
            Background::Dark => Background::Gray,
            Background::Gray => Background::Light,
            Background::Light => Background::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PanelTab {
    #[default]
    Pages,
    Files,
    Markups,
    Find,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// 0-based sheet last viewed.
    pub page: u32,
}

pub const UI_SCALES: [f32; 4] = [0.9, 1.0, 1.15, 1.3];
const MAX_RECENT: usize = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Mouse wheel zooms (Bluebeam default); otherwise it scrolls and
    /// Ctrl+wheel zooms.
    pub wheel_zooms: bool,
    pub snap_default: bool,
    pub units_default: Units,
    pub background: Background,
    pub ui_scale: f32,
    pub reopen_last: bool,
    pub check_updates: bool,
    pub use_pdfium: bool,
    pub panel_open: bool,
    pub panel_tab: PanelTab,
    pub recent: Vec<Recent>,
    pub pinned: Vec<PathBuf>,
    /// Markup style for new markups.
    pub markup_color: u32,
    pub markup_width: f32,
    pub markup_font: f32,
    pub presets: Vec<Preset>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            wheel_zooms: true,
            snap_default: true,
            units_default: Units::FeetInches,
            background: Background::Dark,
            ui_scale: 1.0,
            reopen_last: false,
            check_updates: true,
            use_pdfium: true,
            panel_open: true,
            panel_tab: PanelTab::Pages,
            recent: Vec::new(),
            pinned: Vec::new(),
            markup_color: 0xe53935,
            markup_width: 2.0,
            markup_font: 14.0,
            presets: Vec::new(),
        }
    }
}

/// Case-insensitive path comparison (Windows paths).
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Red-Beam").join("settings.json"))
    }

    pub fn load() -> Settings {
        Self::path().and_then(|p| Self::load_from(&p)).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Option<Settings> {
        let mut s: Settings = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        if !UI_SCALES.iter().any(|v| (v - s.ui_scale).abs() < 1e-3) {
            s.ui_scale = 1.0;
        }
        Some(s)
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = self.save_to(&p);
        }
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Record an opened file at the top of the recent list.
    pub fn touch_recent(&mut self, path: &Path, page: u32) {
        self.recent.retain(|r| !same_path(&r.path, path));
        self.recent.insert(0, Recent { path: path.to_path_buf(), page });
        self.recent.truncate(MAX_RECENT);
    }

    /// Remember the sheet being viewed in a file.
    pub fn set_recent_page(&mut self, path: &Path, page: u32) {
        if let Some(r) = self.recent.iter_mut().find(|r| same_path(&r.path, path)) {
            r.page = page;
        }
    }

    pub fn last_page(&self, path: &Path) -> u32 {
        self.recent.iter().find(|r| same_path(&r.path, path)).map(|r| r.page).unwrap_or(0)
    }

    pub fn is_pinned(&self, path: &Path) -> bool {
        self.pinned.iter().any(|p| same_path(p, path))
    }

    pub fn toggle_pin(&mut self, path: &Path) {
        if self.is_pinned(path) {
            self.pinned.retain(|p| !same_path(p, path));
        } else {
            self.pinned.push(path.to_path_buf());
        }
    }

    pub fn next_ui_scale(&self) -> f32 {
        let i = UI_SCALES.iter().position(|v| (v - self.ui_scale).abs() < 1e-3).unwrap_or(1);
        UI_SCALES[(i + 1) % UI_SCALES.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_and_pins() {
        let mut s = Settings::default();
        s.touch_recent(Path::new(r"C:\Jobs\A.pdf"), 0);
        s.touch_recent(Path::new(r"C:\Jobs\B.pdf"), 3);
        s.touch_recent(Path::new(r"c:\jobs\a.PDF"), 5); // same file, different case
        assert_eq!(s.recent.len(), 2);
        assert_eq!(s.recent[0].page, 5);
        s.set_recent_page(Path::new(r"C:\Jobs\B.pdf"), 7);
        assert_eq!(s.last_page(Path::new(r"C:\JOBS\B.pdf")), 7);
        assert_eq!(s.last_page(Path::new(r"C:\Other.pdf")), 0);
        for i in 0..30 {
            s.touch_recent(Path::new(&format!(r"C:\x{i}.pdf")), 0);
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        s.toggle_pin(Path::new(r"C:\Jobs\A.pdf"));
        assert!(s.is_pinned(Path::new(r"c:\jobs\a.pdf")));
        s.toggle_pin(Path::new(r"C:\JOBS\A.pdf"));
        assert!(s.pinned.is_empty());
        assert_eq!(Settings { ui_scale: 1.3, ..Default::default() }.next_ui_scale(), 0.9);
    }

    #[test]
    fn roundtrip_and_forward_compat() {
        let dir = std::env::temp_dir().join(format!("rb-settings-{}", std::process::id()));
        let p = dir.join("settings.json");
        let mut s = Settings { wheel_zooms: false, background: Background::Light, ui_scale: 1.15, ..Default::default() };
        s.touch_recent(Path::new("C:/a.pdf"), 2);
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p).unwrap(), s);
        // Older/partial files fill in defaults; bad values are corrected.
        std::fs::write(&p, r#"{"wheel_zooms": false, "ui_scale": 7.0}"#).unwrap();
        let o = Settings::load_from(&p).unwrap();
        assert!(!o.wheel_zooms && o.check_updates && o.ui_scale == 1.0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
