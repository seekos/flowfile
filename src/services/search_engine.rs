use super::FileEngine;
use crate::models::{FileItem, home_directory};
use anyhow::{Context as _, Result, bail};
use std::{
    cmp::Reverse,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
    ) -> Result<Vec<FileItem>> {
        self.runtime
            .spawn_blocking(move || {
                if query.trim().is_empty() {
                    return Ok(Vec::new());
                }

                #[cfg(target_os = "macos")]
                if let Ok(results) = spotlight_search(&query, &current_path, scope, show_hidden) {
                    return Ok(results);
                }

                let root = match scope {
                    SearchScope::CurrentFolder => current_path,
                    SearchScope::Everywhere => home_directory(),
                };
                walk_search(&query, &root, show_hidden)
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
) -> Result<Vec<FileItem>> {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
        .replace('?', "\\?");
    let expression = format!("kMDItemFSName == \"*{escaped}*\"cd");
    let mut command = Command::new("/usr/bin/mdfind");
    command.arg("-0");
    if scope == SearchScope::CurrentFolder {
        command.arg("-onlyin").arg(current_path);
    }
    let output = command
        .arg(expression)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("无法启动 Spotlight mdfind")?;
    if !output.status.success() {
        bail!("Spotlight 查询失败");
    }

    let paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| PathBuf::from(String::from_utf8_lossy(bytes).into_owned()));
    Ok(collect_ranked(paths, query, show_hidden))
}

fn walk_search(query: &str, root: &Path, show_hidden: bool) -> Result<Vec<FileItem>> {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(false)
        .into_iter()
        .filter_entry(|entry| show_hidden || !is_hidden_entry(entry));
    let paths = walker
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0)
        .map(|entry| entry.into_path());
    Ok(collect_ranked(paths, query, show_hidden))
}

fn collect_ranked(
    paths: impl IntoIterator<Item = PathBuf>,
    query: &str,
    show_hidden: bool,
) -> Vec<FileItem> {
    let mut ranked = paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let is_hidden = name.starts_with('.');
            if is_hidden && !show_hidden {
                return None;
            }
            let score = fuzzy_score(&name, query)?;
            let metadata = fs::metadata(&path).ok()?;
            Some((
                Reverse(score),
                name.to_lowercase(),
                FileItem::from_metadata(path, name, metadata, is_hidden),
            ))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    ranked
        .into_iter()
        .take(SEARCH_RESULT_LIMIT)
        .map(|(_, _, item)| item)
        .collect()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate = candidate.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(index) = candidate.find(&query) {
        return Some(10_000 - index as i32 * 8 - candidate.len() as i32);
    }

    let mut score = 0_i32;
    let mut last_index = None;
    let chars = candidate.chars().collect::<Vec<_>>();
    let mut cursor = 0;
    for needle in query.chars() {
        let relative = chars[cursor..].iter().position(|value| *value == needle)?;
        let index = cursor + relative;
        score += 120;
        if let Some(previous) = last_index {
            score -= (index - previous - 1) as i32 * 7;
        }
        if index == 0
            || chars
                .get(index.wrapping_sub(1))
                .is_some_and(|value| matches!(value, '-' | '_' | ' ' | '.'))
        {
            score += 40;
        }
        last_index = Some(index);
        cursor = index + 1;
    }
    Some(score - candidate.len() as i32)
}

fn is_hidden_entry(entry: &DirEntry) -> bool {
    entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn contiguous_matches_rank_above_sparse_matches() {
        let contiguous = fuzzy_score("flowfile-search.rs", "search").unwrap();
        let sparse = fuzzy_score("some_rare_archive.rs", "search").unwrap();
        assert!(contiguous > sparse);
    }

    #[test]
    fn rejects_non_matching_names() {
        assert_eq!(fuzzy_score("notes.txt", "xyz"), None);
    }
}
