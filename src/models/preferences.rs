use super::{LayoutMode, home_directory};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    Auto,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::Auto,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPreferences {
    pub theme: ThemePreference,
    pub default_layout: LayoutMode,
    pub show_hidden: bool,
    pub search_shortcut: String,
    pub terminal_shortcut: String,
    pub quick_look_shortcut: String,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Auto,
            default_layout: LayoutMode::Single,
            show_hidden: false,
            search_shortcut: "cmd-f".to_string(),
            terminal_shortcut: "cmd-`".to_string(),
            quick_look_shortcut: "space".to_string(),
        }
    }
}

impl AppPreferences {
    pub fn load() -> Self {
        fs::read(preferences_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = preferences_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(&path, bytes).with_context(|| format!("无法保存设置 {}", path.display()))
    }
}

pub fn preferences_path() -> PathBuf {
    home_directory()
        .join("Library")
        .join("Application Support")
        .join("FlowFile")
        .join("preferences.json")
}

#[cfg(test)]
mod tests {
    use super::AppPreferences;
    use crate::models::LayoutMode;

    #[test]
    fn fresh_install_starts_with_a_single_pane() {
        assert_eq!(AppPreferences::default().default_layout, LayoutMode::Single);
    }
}
