use super::{
    ExplorerTab, LayoutMode, MultiPaneModel, Pane, SortMode, ViewMode, home_directory,
    pane::NAVIGATION_HISTORY_LIMIT,
};
use crate::services::FileEngine;
use anyhow::{Context as _, Result};
use gpui::App;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SESSION_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    pub version: u32,
    pub layout_mode: LayoutMode,
    pub active_pane_index: usize,
    pub last_active_pane_index: Option<usize>,
    pub sidebar_visible: bool,
    pub panes: Vec<PaneSession>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            layout_mode: LayoutMode::default(),
            active_pane_index: 0,
            last_active_pane_index: Some(1),
            sidebar_visible: true,
            panes: Vec::new(),
        }
    }
}

impl SessionState {
    pub fn load() -> Result<Option<Self>> {
        Self::load_from(session_path())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(session_path())
    }

    pub fn capture(model: &MultiPaneModel, sidebar_visible: bool, cx: &App) -> SessionState {
        SessionState {
            version: SESSION_VERSION,
            layout_mode: model.layout_mode,
            active_pane_index: model.active_pane_index,
            last_active_pane_index: model.last_active_pane_index,
            sidebar_visible,
            panes: model
                .panes
                .iter()
                .map(|pane| PaneSession::capture(pane.read(cx)))
                .collect(),
        }
    }

    fn load_from(path: PathBuf) -> Result<Option<Self>> {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("无法读取会话文件 {}", path.display()));
            }
        };
        let mut session: SessionState = serde_json::from_slice(&bytes)
            .with_context(|| format!("无法解析会话文件 {}", path.display()))?;
        session.version = SESSION_VERSION;
        Ok(Some(session))
    }

    fn save_to(&self, path: PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建会话目录 {}", parent.display()))?;
        }

        let bytes = serde_json::to_vec_pretty(self)?;
        let temporary_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
        fs::write(&temporary_path, bytes)
            .with_context(|| format!("无法写入临时会话文件 {}", temporary_path.display()))?;
        fs::rename(&temporary_path, &path)
            .with_context(|| format!("无法更新会话文件 {}", path.display()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneSession {
    pub tabs: Vec<ExplorerTab>,
    pub active_tab_index: usize,
    pub show_hidden: bool,
    pub sort_mode: SortMode,
    pub view_mode: ViewMode,
}

impl Default for PaneSession {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab_index: 0,
            show_hidden: false,
            sort_mode: SortMode::Name,
            view_mode: ViewMode::Grid,
        }
    }
}

impl PaneSession {
    fn capture(pane: &Pane) -> Self {
        Self {
            tabs: pane.tabs.clone(),
            active_tab_index: pane.active_tab_index,
            show_hidden: pane.show_hidden,
            sort_mode: pane.sort_mode,
            view_mode: pane.view_mode,
        }
    }

    pub fn restore(
        &self,
        fallback_path: PathBuf,
        engine: FileEngine,
        default_show_hidden: bool,
    ) -> Pane {
        let fallback_path = valid_directory(fallback_path).unwrap_or_else(|| {
            valid_directory(home_directory()).unwrap_or_else(|| PathBuf::from("/"))
        });
        let mut tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| sanitize_tab(tab, &fallback_path))
            .collect();
        if tabs.is_empty() {
            tabs.push(ExplorerTab::new(fallback_path.clone()));
        }

        let active_tab_index = self.active_tab_index.min(tabs.len() - 1);
        let current_path = tabs[active_tab_index].path.clone();
        let mut pane = Pane::new(current_path, engine);
        pane.tabs = tabs;
        pane.active_tab_index = active_tab_index;
        pane.current_path = pane.active_tab().path.clone();
        pane.show_hidden = if self.tabs.is_empty() {
            default_show_hidden
        } else {
            self.show_hidden
        };
        pane.sort_mode = self.sort_mode;
        pane.view_mode = self.view_mode;
        pane
    }
}

fn sanitize_tab(tab: &ExplorerTab, fallback_path: &Path) -> ExplorerTab {
    let source_history = if tab.history.is_empty() {
        vec![tab.path.clone()]
    } else {
        tab.history.clone()
    };
    let old_index = tab
        .history_index
        .min(source_history.len().saturating_sub(1));
    let mut history = Vec::new();
    let mut history_index = 0;

    for (index, path) in source_history.into_iter().enumerate() {
        if let Some(path) = valid_directory(path) {
            history.push(path);
            if index <= old_index {
                history_index = history.len() - 1;
            }
        }
    }

    if history.is_empty() {
        return ExplorerTab::new(fallback_path.to_path_buf());
    }

    if history.len() > NAVIGATION_HISTORY_LIMIT {
        let latest_start = history.len() - NAVIGATION_HISTORY_LIMIT;
        let centered_start = history_index.saturating_sub(NAVIGATION_HISTORY_LIMIT / 2);
        let start = centered_start.min(latest_start);
        history = history
            .into_iter()
            .skip(start)
            .take(NAVIGATION_HISTORY_LIMIT)
            .collect();
        history_index -= start;
    }

    let path = history[history_index].clone();
    let mut restored = ExplorerTab::new(path.clone());
    restored.path = path;
    restored.history = history;
    restored.history_index = history_index;
    restored
}

fn valid_directory(path: PathBuf) -> Option<PathBuf> {
    fs::metadata(&path)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .map(|_| path)
}

pub fn session_path() -> PathBuf {
    home_directory()
        .join("Library")
        .join("Application Support")
        .join("FlowFile")
        .join("session.json")
}

#[cfg(test)]
mod tests {
    use super::{NAVIGATION_HISTORY_LIMIT, PaneSession, SessionState, sanitize_tab};
    use crate::models::{ExplorerTab, LayoutMode, SortMode, ViewMode};
    use std::{fs, path::PathBuf};

    #[test]
    fn new_session_defaults_to_single_pane_grid_view() {
        assert_eq!(SessionState::default().layout_mode, LayoutMode::Single);
        assert_eq!(PaneSession::default().view_mode, ViewMode::Grid);
    }

    #[test]
    fn restored_history_drops_missing_directories_and_preserves_position() {
        let directory = tempfile::tempdir().expect("temp directory");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs::create_dir(&first).expect("first directory");
        fs::create_dir(&second).expect("second directory");
        let missing = directory.path().join("missing");
        let tab = ExplorerTab {
            title: "missing".to_string(),
            path: missing.clone(),
            history: vec![first.clone(), missing, second.clone()],
            history_index: 1,
        };

        let restored = sanitize_tab(&tab, &first);

        assert_eq!(restored.history, vec![first.clone(), second]);
        assert_eq!(restored.history_index, 0);
        assert_eq!(restored.path, first);
    }

    #[test]
    fn restored_history_is_capped_without_losing_the_current_position() {
        let directory = tempfile::tempdir().expect("temp directory");
        let valid = directory.path().to_path_buf();
        let history = (0..NAVIGATION_HISTORY_LIMIT + 40)
            .map(|_| valid.clone())
            .collect::<Vec<_>>();
        let tab = ExplorerTab {
            title: "history".to_string(),
            path: valid.clone(),
            history,
            history_index: 70,
        };

        let restored = sanitize_tab(&tab, &valid);

        assert_eq!(restored.history.len(), NAVIGATION_HISTORY_LIMIT);
        assert_eq!(restored.history_index, NAVIGATION_HISTORY_LIMIT / 2);
        assert_eq!(restored.path, valid);
    }

    #[test]
    fn session_file_round_trips() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("session.json");
        let session = SessionState {
            layout_mode: LayoutMode::Quad,
            active_pane_index: 3,
            last_active_pane_index: Some(1),
            sidebar_visible: false,
            panes: vec![PaneSession {
                tabs: vec![ExplorerTab::new(PathBuf::from("/"))],
                active_tab_index: 0,
                show_hidden: true,
                sort_mode: SortMode::Modified,
                view_mode: ViewMode::Grid,
            }],
            ..SessionState::default()
        };

        session.save_to(path.clone()).expect("save session");
        let restored = SessionState::load_from(path)
            .expect("load session")
            .expect("session exists");

        assert_eq!(restored, session);
    }
}
