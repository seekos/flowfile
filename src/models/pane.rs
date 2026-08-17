use super::{FileItem, SortMode};
use crate::services::{
    DirectorySnapshot, FileEngine, FileOperationEngine, FileWatcher, SearchEngine, SearchScope,
};
use gpui::{Context, Timer};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) const NAVIGATION_HISTORY_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ViewMode {
    Details,
    #[default]
    Grid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplorerTab {
    pub title: String,
    pub path: PathBuf,
    pub history: Vec<PathBuf>,
    pub history_index: usize,
}

impl ExplorerTab {
    pub fn new(path: PathBuf) -> Self {
        Self {
            title: path_title(&path),
            path: path.clone(),
            history: vec![path],
            history_index: 0,
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    fn push_path(&mut self, path: PathBuf) {
        if self.path == path {
            return;
        }

        self.history.truncate(self.history_index + 1);
        self.history.push(path.clone());
        if self.history.len() > NAVIGATION_HISTORY_LIMIT {
            let overflow = self.history.len() - NAVIGATION_HISTORY_LIMIT;
            self.history.drain(..overflow);
        }
        self.history_index = self.history.len() - 1;
        self.path = path.clone();
        self.title = path_title(&path);
    }

    fn move_to_history(&mut self, index: usize, path: PathBuf) {
        self.history_index = index;
        self.path = path.clone();
        self.title = path_title(&path);
    }
}

enum NavigationIntent {
    Push,
    History(usize),
    Refresh,
}

pub struct Pane {
    pub current_path: PathBuf,
    pub tabs: Vec<ExplorerTab>,
    pub active_tab_index: usize,
    pub items: Vec<FileItem>,
    pub show_hidden: bool,
    pub sort_mode: SortMode,
    pub view_mode: ViewMode,
    pub selected_index: Option<usize>,
    pub selected_indices: BTreeSet<usize>,
    pub rename_index: Option<usize>,
    pub rename_buffer: String,
    rename_in_progress: bool,
    pub is_loading: bool,
    pub error_message: Option<String>,
    pub search_active: bool,
    pub search_query: String,
    pub search_scope: SearchScope,
    pub search_result_count: usize,
    engine: FileEngine,
    search_engine: SearchEngine,
    operation_engine: FileOperationEngine,
    search_original_items: Vec<FileItem>,
    search_generation: u64,
    load_generation: u64,
    watcher_generation: u64,
    watcher: Option<FileWatcher>,
}

impl Pane {
    pub fn new(path: PathBuf, engine: FileEngine) -> Self {
        let operation_engine = FileOperationEngine::new(&engine);
        let search_engine = SearchEngine::new(&engine);
        Self {
            current_path: path.clone(),
            tabs: vec![ExplorerTab::new(path)],
            active_tab_index: 0,
            items: Vec::new(),
            show_hidden: false,
            sort_mode: SortMode::Name,
            view_mode: ViewMode::Grid,
            selected_index: None,
            selected_indices: BTreeSet::new(),
            rename_index: None,
            rename_buffer: String::new(),
            rename_in_progress: false,
            is_loading: false,
            error_message: None,
            search_active: false,
            search_query: String::new(),
            search_scope: SearchScope::CurrentFolder,
            search_result_count: 0,
            engine,
            search_engine,
            operation_engine,
            search_original_items: Vec::new(),
            search_generation: 0,
            load_generation: 0,
            watcher_generation: 0,
            watcher: None,
        }
    }

    pub fn active_tab(&self) -> &ExplorerTab {
        &self.tabs[self.active_tab_index]
    }

    fn active_tab_mut(&mut self) -> &mut ExplorerTab {
        &mut self.tabs[self.active_tab_index]
    }

    pub fn can_go_back(&self) -> bool {
        self.active_tab().can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.active_tab().can_go_forward()
    }

    pub fn can_go_up(&self) -> bool {
        self.current_path.parent().is_some()
    }

    pub fn load_initial(&mut self, cx: &mut Context<Self>) {
        self.load_path(self.current_path.clone(), NavigationIntent::Refresh, cx);
    }

    pub fn navigate_to(&mut self, input: PathBuf, cx: &mut Context<Self>) {
        self.cancel_search_state();
        let path = resolve_path(&self.current_path, input);
        self.load_path(path, NavigationIntent::Push, cx);
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        self.cancel_search_state();
        let tab = self.active_tab();
        if tab.can_go_back() {
            let index = tab.history_index - 1;
            self.load_path(
                tab.history[index].clone(),
                NavigationIntent::History(index),
                cx,
            );
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        self.cancel_search_state();
        let tab = self.active_tab();
        if tab.can_go_forward() {
            let index = tab.history_index + 1;
            self.load_path(
                tab.history[index].clone(),
                NavigationIntent::History(index),
                cx,
            );
        }
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        self.cancel_search_state();
        if let Some(parent) = self.current_path.parent() {
            self.load_path(parent.to_path_buf(), NavigationIntent::Push, cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_path(self.current_path.clone(), NavigationIntent::Refresh, cx);
    }

    pub fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.refresh(cx);
    }

    pub fn set_sort_mode(&mut self, sort_mode: SortMode, cx: &mut Context<Self>) {
        if self.sort_mode == sort_mode {
            return;
        }
        self.sort_mode = sort_mode;
        self.refresh(cx);
    }

    pub fn set_view_mode(&mut self, view_mode: ViewMode, cx: &mut Context<Self>) {
        if self.view_mode != view_mode {
            self.view_mode = view_mode;
            cx.notify();
        }
    }

    pub fn begin_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_active {
            self.search_original_items = self.items.clone();
            self.search_active = true;
            self.search_query.clear();
            self.search_scope = SearchScope::CurrentFolder;
            self.search_result_count = self.items.len();
            self.selected_index = None;
            self.selected_indices.clear();
            self.rename_index = None;
            cx.notify();
        }
    }

    pub fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.schedule_search(cx);
    }

    pub fn toggle_search_scope(&mut self, cx: &mut Context<Self>) {
        self.search_scope = match self.search_scope {
            SearchScope::CurrentFolder => SearchScope::Everywhere,
            SearchScope::Everywhere => SearchScope::CurrentFolder,
        };
        self.schedule_search(cx);
    }

    pub fn exit_search(&mut self, cx: &mut Context<Self>) {
        if self.search_active {
            self.search_generation += 1;
            self.items = std::mem::take(&mut self.search_original_items);
            self.search_active = false;
            self.search_query.clear();
            self.search_result_count = 0;
            self.selected_index = None;
            self.selected_indices.clear();
            self.error_message = None;
            cx.notify();
        }
    }

    fn cancel_search_state(&mut self) {
        if self.search_active {
            self.search_generation += 1;
            self.search_active = false;
            self.search_query.clear();
            self.search_original_items.clear();
            self.search_result_count = 0;
        }
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.search_query.clone();
        if query.trim().is_empty() {
            self.items = self.search_original_items.clone();
            self.search_result_count = self.items.len();
            self.is_loading = false;
            self.error_message = None;
            cx.notify();
            return;
        }

        let engine = self.search_engine.clone();
        let current_path = self.current_path.clone();
        let scope = self.search_scope;
        let show_hidden = self.show_hidden;
        self.is_loading = true;
        self.error_message = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            let still_current = this
                .update(cx, |pane, _| {
                    pane.search_active && pane.search_generation == generation
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            let result = engine.search(query, current_path, scope, show_hidden).await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.search_active || pane.search_generation != generation {
                    return;
                }
                pane.is_loading = false;
                match result {
                    Ok(items) => {
                        pane.search_result_count = items.len();
                        pane.items = items;
                        pane.selected_index = None;
                        pane.selected_indices.clear();
                    }
                    Err(error) => pane.error_message = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn select(&mut self, index: usize, additive: bool, range: bool) {
        if index >= self.items.len() {
            return;
        }

        if range {
            let anchor = self.selected_index.unwrap_or(index);
            self.selected_indices.clear();
            for selected in anchor.min(index)..=anchor.max(index) {
                self.selected_indices.insert(selected);
            }
        } else if additive {
            if !self.selected_indices.remove(&index) {
                self.selected_indices.insert(index);
            }
        } else {
            self.selected_indices.clear();
            self.selected_indices.insert(index);
        }

        if self.selected_indices.contains(&index) {
            self.selected_index = Some(index);
        } else {
            self.selected_index = self.selected_indices.iter().next_back().copied();
        }
        self.rename_index = None;
    }

    pub fn select_for_context_menu(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        if !self.selected_indices.contains(&index) {
            self.selected_indices.clear();
            self.selected_indices.insert(index);
        }
        self.selected_index = Some(index);
        self.rename_index = None;
    }

    pub fn selection_count(&self) -> usize {
        self.selected_indices.len()
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_indices
            .iter()
            .filter_map(|index| self.items.get(*index))
            .map(|item| item.path.clone())
            .collect()
    }

    pub fn clear_selection(&mut self) {
        self.selected_index = None;
        self.selected_indices.clear();
        self.rename_index = None;
        self.rename_buffer.clear();
    }

    pub fn set_selection_indices(&mut self, indices: BTreeSet<usize>) -> bool {
        let indices = indices
            .into_iter()
            .filter(|index| *index < self.items.len())
            .collect::<BTreeSet<_>>();
        if self.selected_indices == indices {
            return false;
        }
        self.selected_indices = indices;
        self.selected_index = self.selected_indices.iter().next_back().copied();
        self.rename_index = None;
        self.rename_buffer.clear();
        true
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected_index = None;
            return;
        }

        let current = self.selected_index.unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.items.len() as isize - 1) as usize;
        self.selected_index = Some(next);
        self.selected_indices.clear();
        self.selected_indices.insert(next);
        self.rename_index = None;
    }

    pub fn activate_selected(&mut self, cx: &mut Context<Self>) {
        if self.rename_index.is_some() {
            return;
        }
        let Some(item) = self
            .selected_index
            .and_then(|index| self.items.get(index))
            .cloned()
        else {
            return;
        };

        if should_navigate_to(&item) {
            self.navigate_to(item.path, cx);
        } else {
            self.open_file(item.path, cx);
        }
    }

    pub fn begin_rename(&mut self) {
        if self.selection_count() != 1 {
            return;
        }
        let Some(index) = self.selected_indices.iter().next().copied() else {
            return;
        };
        let Some(item) = self.items.get(index) else {
            return;
        };
        self.rename_index = Some(index);
        self.rename_buffer = item.name.clone();
        self.rename_in_progress = false;
        self.error_message = None;
    }

    pub fn cancel_rename(&mut self) {
        self.rename_index = None;
        self.rename_buffer.clear();
        self.rename_in_progress = false;
    }

    pub fn append_rename_text(&mut self, text: &str) {
        self.rename_buffer.push_str(text);
    }

    pub fn set_rename_buffer(&mut self, value: String) {
        if self.rename_index.is_some() {
            self.rename_buffer = value;
        }
    }

    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if self.rename_in_progress {
            return;
        }
        let Some(index) = self.rename_index else {
            self.begin_rename();
            cx.notify();
            return;
        };
        let Some(source) = self.items.get(index).map(|item| item.path.clone()) else {
            self.cancel_rename();
            return;
        };
        let new_name = self.rename_buffer.trim().to_string();
        let engine = self.operation_engine.clone();
        self.rename_in_progress = true;

        cx.spawn(async move |this, cx| {
            let result = engine.rename(source, new_name).await;
            let _ = this.update(cx, |pane, cx| match result {
                Ok(_) => {
                    pane.cancel_rename();
                    pane.refresh(cx);
                }
                Err(error) => {
                    pane.rename_in_progress = false;
                    pane.error_message = Some(error.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = engine.open_path(path).await {
                let _ = this.update(cx, |pane, cx| {
                    pane.error_message = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn load_path(&mut self, path: PathBuf, intent: NavigationIntent, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let generation = self.load_generation;
        let engine = self.engine.clone();
        let show_hidden = self.show_hidden;
        let sort_mode = self.sort_mode;

        self.is_loading = true;
        self.error_message = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = engine
                .read_directory(path.clone(), show_hidden, sort_mode)
                .await;

            let _ = this.update(cx, |pane, cx| {
                if pane.load_generation != generation {
                    return;
                }

                pane.is_loading = false;
                match result {
                    Ok(snapshot) => {
                        let path_changed = pane.current_path != snapshot.path;
                        pane.apply_snapshot(snapshot, intent);
                        if path_changed || pane.watcher.is_none() {
                            pane.start_watching(cx);
                        }
                    }
                    Err(error) => {
                        pane.error_message = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snapshot: DirectorySnapshot, intent: NavigationIntent) {
        if self.search_active && matches!(&intent, NavigationIntent::Refresh) {
            self.search_original_items = snapshot.items;
            return;
        }
        let selected_paths = if matches!(&intent, NavigationIntent::Refresh) {
            self.selected_paths().into_iter().collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };

        match intent {
            NavigationIntent::Push => self.active_tab_mut().push_path(snapshot.path.clone()),
            NavigationIntent::History(index) => self
                .active_tab_mut()
                .move_to_history(index, snapshot.path.clone()),
            NavigationIntent::Refresh => {
                let tab = self.active_tab_mut();
                tab.path = snapshot.path.clone();
                tab.title = path_title(&snapshot.path);
            }
        }
        self.current_path = snapshot.path;
        self.items = snapshot.items;
        self.selected_indices = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| selected_paths.contains(&item.path))
            .map(|(index, _)| index)
            .collect();
        self.selected_index = self.selected_indices.iter().next_back().copied();
        self.rename_index = None;
        self.rename_in_progress = false;
    }

    fn start_watching(&mut self, cx: &mut Context<Self>) {
        self.watcher_generation += 1;
        let generation = self.watcher_generation;
        let (watcher, receiver) = match FileWatcher::watch(&self.current_path) {
            Ok(value) => value,
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.watcher = None;
                return;
            }
        };
        self.watcher = Some(watcher);

        cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                Timer::after(Duration::from_millis(150)).await;
                while receiver.try_recv().is_ok() {}

                let active = this
                    .update(cx, |pane, _| pane.watcher_generation == generation)
                    .unwrap_or(false);
                if !active {
                    break;
                }

                let _ = this.update(cx, |pane, cx| pane.refresh(cx));
            }
        })
        .detach();
    }
}

fn path_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
        .to_string()
}

fn should_navigate_to(item: &FileItem) -> bool {
    item.is_dir
        && !item
            .extension
            .as_deref()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

fn resolve_path(current: &Path, input: PathBuf) -> PathBuf {
    let display = input.to_string_lossy();
    if display == "~" {
        return home_directory();
    }
    if let Some(rest) = display.strip_prefix("~/") {
        return home_directory().join(rest);
    }
    if input.is_absolute() {
        input
    } else {
        current.join(input)
    }
}

pub fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::{ExplorerTab, NAVIGATION_HISTORY_LIMIT, Pane, ViewMode, should_navigate_to};
    use crate::{
        models::{FileItem, FileKind},
        services::FileEngine,
    };
    use std::{collections::BTreeSet, path::PathBuf};

    #[test]
    fn new_panes_start_in_grid_view() {
        let pane = Pane::new(
            PathBuf::from("/tmp"),
            FileEngine::new().expect("file engine"),
        );

        assert_eq!(pane.view_mode, ViewMode::Grid);
    }

    #[test]
    fn pushing_a_path_truncates_forward_history() {
        let home = PathBuf::from("/Users/test");
        let mut tab = ExplorerTab::new(home.clone());
        tab.push_path(home.join("Downloads"));
        tab.push_path(home.join("Desktop"));
        tab.move_to_history(1, home.join("Downloads"));
        tab.push_path(home.join("Documents"));

        assert_eq!(tab.history.len(), 3);
        assert_eq!(tab.history_index, 2);
        assert_eq!(tab.history[2], home.join("Documents"));
    }

    #[test]
    fn navigation_history_discards_the_oldest_entries_at_the_limit() {
        let root = PathBuf::from("/history");
        let mut tab = ExplorerTab::new(root.clone());
        for index in 0..NAVIGATION_HISTORY_LIMIT + 20 {
            tab.push_path(root.join(index.to_string()));
        }

        assert_eq!(tab.history.len(), NAVIGATION_HISTORY_LIMIT);
        assert_eq!(tab.history_index, NAVIGATION_HISTORY_LIMIT - 1);
        assert_eq!(
            tab.path,
            root.join((NAVIGATION_HISTORY_LIMIT + 19).to_string())
        );
        assert_eq!(tab.history[0], root.join("20"));
    }

    #[test]
    fn context_click_preserves_existing_multi_selection_and_tracks_clicked_item() {
        let mut pane = Pane::new(
            PathBuf::from("/tmp"),
            FileEngine::new().expect("file engine"),
        );
        pane.items = (0..3)
            .map(|index| FileItem {
                path: PathBuf::from(format!("/tmp/{index}.txt")),
                name: format!("{index}.txt"),
                is_dir: false,
                extension: Some("txt".to_string()),
                size: 0,
                modified_unix: 0,
                modified: String::new(),
                is_hidden: false,
                kind: FileKind::Document,
            })
            .collect();
        pane.select(0, false, false);
        pane.select(1, true, false);

        pane.select_for_context_menu(0);
        assert_eq!(
            pane.selected_indices.iter().copied().collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(pane.selected_index, Some(0));

        pane.select_for_context_menu(2);
        assert_eq!(
            pane.selected_indices.iter().copied().collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(pane.selected_index, Some(2));
    }

    #[test]
    fn marquee_selection_filters_indices_outside_the_current_directory() {
        let mut pane = Pane::new(
            PathBuf::from("/tmp"),
            FileEngine::new().expect("file engine"),
        );
        pane.items = (0..3)
            .map(|index| FileItem {
                path: PathBuf::from(format!("/tmp/{index}.txt")),
                name: format!("{index}.txt"),
                is_dir: false,
                extension: Some("txt".to_string()),
                size: 0,
                modified_unix: 0,
                modified: String::new(),
                is_hidden: false,
                kind: FileKind::Document,
            })
            .collect();

        assert!(pane.set_selection_indices(BTreeSet::from([0, 2, 9])));
        assert_eq!(pane.selected_indices, BTreeSet::from([0, 2]));
        assert_eq!(pane.selected_index, Some(2));
        assert!(!pane.set_selection_indices(BTreeSet::from([0, 2])));
    }

    #[test]
    fn rename_buffer_accepts_chinese_folder_names() {
        let mut pane = Pane::new(
            PathBuf::from("/tmp"),
            FileEngine::new().expect("file engine"),
        );
        pane.items = vec![FileItem {
            path: PathBuf::from("/tmp/folder"),
            name: "folder".to_string(),
            is_dir: true,
            extension: None,
            size: 0,
            modified_unix: 0,
            modified: String::new(),
            is_hidden: false,
            kind: FileKind::Folder,
        }];
        pane.select(0, false, false);
        pane.begin_rename();

        pane.set_rename_buffer("中文文件夹".to_string());

        assert_eq!(pane.rename_buffer, "中文文件夹");
    }

    #[test]
    fn application_bundles_open_instead_of_navigate() {
        let application = FileItem {
            path: PathBuf::from("/Applications/Example.app"),
            name: "Example.app".to_string(),
            is_dir: true,
            extension: Some("app".to_string()),
            size: 0,
            modified_unix: 0,
            modified: String::new(),
            is_hidden: false,
            kind: FileKind::Folder,
        };
        let mut folder = application.clone();
        folder.path = PathBuf::from("/Applications/Utilities");
        folder.name = "Utilities".to_string();
        folder.extension = None;

        assert!(!should_navigate_to(&application));
        assert!(should_navigate_to(&folder));
    }
}
