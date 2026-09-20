use super::FileEngine;
use crate::models::{FileItem, home_directory};
use anyhow::{Context as _, Result, bail};
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use tokio::runtime::Handle;
use walkdir::{DirEntry, WalkDir};

const SEARCH_RESULT_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchScope {
    #[default]
    CurrentFolder,
    Everywhere,
}

impl SearchScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentFolder => "当前目录",
            Self::Everywhere => "整台 Mac",
        }
    }
}

#[derive(Clone)]
pub struct SearchEngine {
    runtime: Handle,
}

impl SearchEngine {
    pub fn new(engine: &FileEngine) -> Self {
        Self {
            runtime: engine.runtime_handle(),
        }
    }

    pub async fn search(
        &self,
        query: String,
        current_path: PathBuf,
        scope: SearchScope,
        show_hidden: bool,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<FileItem>> {
        self.runtime
            .spawn_blocking(move || {
                if query.trim().is_empty() {
                    return Ok(Vec::new());
                }

                #[cfg(target_os = "macos")]
                if let Ok(results) =
                    spotlight_search(&query, &current_path, scope, show_hidden, &cancelled)
                {
                    return Ok(results);
                }

                ensure_not_cancelled(&cancelled)?;

                let root = match scope {
                    SearchScope::CurrentFolder => current_path,
                    SearchScope::Everywhere => home_directory(),
                };
                walk_search(&query, &root, show_hidden, &cancelled)
            })
            .await
            .context("搜索任务异常终止")?
    }
}

#[cfg(target_os = "macos")]
fn spotlight_search(
    query: &str,
    current_path: &Path,
    scope: SearchScope,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<FileItem>> {
    let pattern = spotlight_name_pattern(query);
    let expression = format!("kMDItemFSName == \"{pattern}\"cd");
    let mut command = Command::new("/usr/bin/mdfind");
    command.arg("-0");
    if scope == SearchScope::CurrentFolder {
        command.arg("-onlyin").arg(current_path);
    }
    let mut child = command
        .arg(expression)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动 Spotlight mdfind")?;
    let mut stdout = child.stdout.take().context("无法读取 Spotlight 结果")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("搜索已取消");
        }
        if let Some(status) = child.try_wait().context("无法等待 Spotlight 查询")? {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Spotlight 结果读取线程异常终止"))??;
    if !status.success() {
        bail!("Spotlight 查询失败");
    }

    let paths = stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| PathBuf::from(String::from_utf8_lossy(bytes).into_owned()));
    collect_ranked(paths, query, show_hidden, cancelled)
}

fn spotlight_name_pattern(query: &str) -> String {
    let mut pattern = String::from("*");
    for character in query.trim().chars() {
        if matches!(character, '\\' | '"' | '*' | '?') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('*');
    pattern
}

fn walk_search(
    query: &str,
    root: &Path,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<FileItem>> {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(false)
        .into_iter()
        .filter_entry(|entry| show_hidden || !is_hidden_entry(entry));
    let paths = walker
        .take_while(|_| !cancelled.load(Ordering::Relaxed))
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0)
        .map(|entry| entry.into_path());
    collect_ranked(paths, query, show_hidden, cancelled)
}

fn collect_ranked(
    paths: impl IntoIterator<Item = PathBuf>,
    query: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<FileItem>> {
    ensure_not_cancelled(cancelled)?;
    let mut ranked = BinaryHeap::with_capacity(SEARCH_RESULT_LIMIT + 1);
    for path in paths {
        ensure_not_cancelled(cancelled)?;
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let is_hidden = name.starts_with('.');
        if is_hidden && !show_hidden {
            continue;
        }
        let Some(score) = fuzzy_score(&name, query) else {
            continue;
        };
        let candidate = (Reverse(score), name.to_lowercase(), path, is_hidden, name);
        if ranked.len() < SEARCH_RESULT_LIMIT {
            ranked.push(candidate);
        } else if ranked.peek().is_some_and(|worst| &candidate < worst) {
            ranked.pop();
            ranked.push(candidate);
        }
    }
    ensure_not_cancelled(cancelled)?;

    let mut ranked = ranked.into_vec();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(ranked
        .into_iter()
        .filter_map(|(_, _, path, is_hidden, name)| {
            let metadata = fs::metadata(&path).ok()?;
            Some(FileItem::from_metadata(path, name, metadata, is_hidden))
        })
        .collect())
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        bail!("搜索已取消");
    }
    Ok(())
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate = candidate.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let index = candidate.find(&query)?;
    Some(10_000 - index as i32 * 8 - candidate.len() as i32)
}

fn is_hidden_entry(entry: &DirEntry) -> bool {
    entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::{SEARCH_RESULT_LIMIT, collect_ranked, fuzzy_score, spotlight_name_pattern};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn contiguous_matches_rank_prefixes_above_later_occurrences() {
        let prefix = fuzzy_score("search-results.rs", "search").unwrap();
        let later = fuzzy_score("flowfile-search.rs", "search").unwrap();
        assert!(prefix > later);
    }

    #[test]
    fn rejects_sparse_character_matches() {
        assert_eq!(fuzzy_score("some_rare_archive.rs", "search"), None);
    }

    #[test]
    fn rejects_non_matching_names() {
        assert_eq!(fuzzy_score("notes.txt", "xyz"), None);
    }

    #[test]
    fn spotlight_pattern_requests_contiguous_name_matches() {
        assert_eq!(spotlight_name_pattern(" lcapi "), "*lcapi*");
        assert_eq!(spotlight_name_pattern(r#"a*?\"b"#), r#"*a\*\?\\\"b*"#);
    }

    #[test]
    fn cancellation_stops_result_collection() {
        let cancelled = AtomicBool::new(true);
        let result = collect_ranked(Vec::new(), "flow", false, &cancelled);
        assert!(result.is_err());
    }

    #[test]
    fn ranked_results_are_bounded_before_metadata_loading() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let paths = (0..SEARCH_RESULT_LIMIT + 25)
            .map(|index| {
                let path = directory.path().join(format!("flow-{index:04}.txt"));
                std::fs::write(&path, b"test").expect("write search candidate");
                path
            })
            .collect::<Vec<_>>();
        let cancelled = AtomicBool::new(false);

        let results = collect_ranked(paths, "flow", false, &cancelled).expect("collect results");

        assert_eq!(results.len(), SEARCH_RESULT_LIMIT);
    }
}
