use super::{LayoutMode, home_directory, persistence};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DefaultApplication {
    pub extension: String,
    pub application: PathBuf,
}

impl DefaultApplication {
    pub fn new(extension: &str, application: PathBuf) -> Result<Self> {
        let extension = normalize_extension(extension)?;
        Ok(Self {
            extension,
            application,
        })
    }

    pub fn application_name(&self) -> String {
        self.application
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("未知应用")
            .to_string()
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
    pub default_applications: Vec<DefaultApplication>,
    pub dismissed_update_version: Option<String>,
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
            default_applications: Vec::new(),
            dismissed_update_version: None,
        }
    }
}

impl AppPreferences {
    pub fn load() -> Self {
        persistence::load_json_or_default(preferences_path(), "设置")
    }

    pub fn save(&self) -> Result<()> {
        let path = preferences_path();
        persistence::atomic_write_json(&path, self, "设置")
    }

    pub fn set_default_application(&mut self, extension: &str, application: PathBuf) -> Result<()> {
        let preference = DefaultApplication::new(extension, application)?;
        if let Some(existing) = self
            .default_applications
            .iter_mut()
            .find(|existing| existing.extension == preference.extension)
        {
            *existing = preference;
        } else {
            self.default_applications.push(preference);
            self.default_applications
                .sort_by(|left, right| left.extension.cmp(&right.extension));
        }
        Ok(())
    }

    pub fn set_default_application_for_path(
        &mut self,
        path: &Path,
        application: PathBuf,
    ) -> Result<bool> {
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return Ok(false);
        };
        self.set_default_application(extension, application)?;
        Ok(true)
    }

    pub fn default_application_for_path(&self, path: &Path) -> Option<&Path> {
        let extension = path.extension()?.to_str()?;
        self.default_applications
            .iter()
            .find(|preference| preference.extension.eq_ignore_ascii_case(extension))
            .map(|preference| preference.application.as_path())
    }
}

fn normalize_extension(extension: &str) -> Result<String> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    if extension.is_empty() {
        bail!("请输入文件后缀名");
    }
    if extension.chars().count() > 32 {
        bail!("文件后缀名不能超过 32 个字符");
    }
    if !extension
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | '+'))
    {
        bail!("文件后缀名只能包含字母、数字、-、_ 或 +");
    }
    Ok(extension)
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
    use super::{AppPreferences, DefaultApplication};
    use crate::models::LayoutMode;
    use std::path::{Path, PathBuf};

    #[test]
    fn fresh_install_starts_with_a_single_pane() {
        assert_eq!(AppPreferences::default().default_layout, LayoutMode::Single);
    }

    #[test]
    fn existing_preferences_without_default_applications_still_load() {
        let preferences: AppPreferences =
            serde_json::from_str("{}").expect("older preferences remain compatible");

        assert!(preferences.default_applications.is_empty());
    }

    #[test]
    fn default_application_extensions_are_normalized_and_replaced() {
        let mut preferences = AppPreferences::default();
        preferences
            .set_default_application(" . T X T ", PathBuf::from("/Applications/TextEdit.app"))
            .expect("valid extension");
        preferences
            .set_default_application("txt", PathBuf::from("/Applications/Code.app"))
            .expect("replacement extension");

        assert_eq!(preferences.default_applications.len(), 1);
        assert_eq!(preferences.default_applications[0].extension, "txt");
        assert_eq!(
            preferences.default_application_for_path(Path::new("README.TXT")),
            Some(Path::new("/Applications/Code.app"))
        );
    }

    #[test]
    fn invalid_default_application_extension_is_rejected() {
        let error =
            DefaultApplication::new("tar.gz", PathBuf::from("/Applications/Archive Utility.app"))
                .expect_err("compound suffix should be rejected");

        assert!(error.to_string().contains("只能包含"));
    }

    #[test]
    fn custom_open_with_adds_the_file_extension_to_defaults() {
        let mut preferences = AppPreferences::default();

        assert!(
            preferences
                .set_default_application_for_path(
                    Path::new("movie.MP4"),
                    PathBuf::from("/Applications/mpv.app"),
                )
                .expect("save custom application")
        );
        assert_eq!(
            preferences.default_application_for_path(Path::new("next.mp4")),
            Some(Path::new("/Applications/mpv.app"))
        );
    }
}
