use crate::models::{FileItem, GitStatus};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) fn annotate_directory(path: &Path, items: &mut [FileItem]) {
    for item in items.iter_mut() {
        item.git_status = None;
    }

    if !annotate_worktree(path, items) {
        annotate_child_repositories(items);
    }
}

pub(crate) fn metadata_directories(path: &Path) -> Vec<PathBuf> {
    for ancestor in path.ancestors() {
        if let Some(directory) = metadata_directory(ancestor) {
            return vec![directory];
        }
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|child| child.is_dir())
        .filter_map(|child| metadata_directory(&child))
        .collect()
}

fn annotate_worktree(path: &Path, items: &mut [FileItem]) -> bool {
    let Some(statuses) = git_status(path) else {
        return false;
    };

    let mut item_indices = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            item.path
                .file_name()
                .map(|name| (name.to_os_string(), index))
        })
        .collect::<HashMap<_, _>>();

    for item in items.iter_mut() {
        if item.name != ".git" && (!item.is_dir || !directory_is_empty(&item.path)) {
            item.git_status = Some(GitStatus::Clean);
        }
    }
    for (code, path) in statuses {
        if code == *b"!!" {
            if is_direct_child(&path) {
                set_status_for_path(&path, None, items, &mut item_indices);
            }
        } else {
            set_status_for_path(&path, Some(GitStatus::Modified), items, &mut item_indices);
        }
    }
    true
}

fn annotate_child_repositories(items: &mut [FileItem]) {
    for item in items.iter_mut().filter(|item| item.is_dir) {
        if !item.path.join(".git").exists() {
            continue;
        }
        let Some(statuses) = git_status(&item.path) else {
            continue;
        };
        item.git_status = Some(if statuses.iter().any(|(code, _)| code != b"!!") {
            GitStatus::Modified
        } else {
            GitStatus::Clean
        });
    }
}

fn set_status_for_path(
    path: &Path,
    status: Option<GitStatus>,
    items: &mut [FileItem],
    item_indices: &mut HashMap<OsString, usize>,
) {
    let Some(name) = path.components().find_map(|component| match component {
        Component::Normal(name) => Some(name.to_os_string()),
        _ => None,
    }) else {
        return;
    };
    if let Some(index) = item_indices.get(&name).copied() {
        items[index].git_status = status;
    }
}

fn git_status(path: &Path) -> Option<Vec<([u8; 2], PathBuf)>> {
    let output = git_output(
        path,
        &[
            OsStr::new("-c"),
            OsStr::new("status.relativePaths=true"),
            OsStr::new("status"),
            OsStr::new("--porcelain=v1"),
            OsStr::new("-z"),
            OsStr::new("--untracked-files=all"),
            OsStr::new("--ignored=matching"),
            OsStr::new("--"),
            OsStr::new("."),
        ],
    )?;
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut statuses = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let code = [record[0], record[1]];
        statuses.push((code, path_from_bytes(&record[3..])));
        if code.iter().any(|byte| matches!(byte, b'R' | b'C'))
            && let Some(source) = records.next()
        {
            statuses.push((code, path_from_bytes(source)));
        }
    }
    Some(statuses)
}

fn is_direct_child(path: &Path) -> bool {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
        == 1
}

fn directory_is_empty(path: &Path) -> bool {
    std::fs::read_dir(path)
        .ok()
        .is_some_and(|mut entries| entries.next().is_none())
}

fn metadata_directory(path: &Path) -> Option<PathBuf> {
    let marker = path.join(".git");
    if marker.is_dir() {
        return Some(marker);
    }
    let contents = std::fs::read_to_string(&marker).ok()?;
    let directory = PathBuf::from(contents.trim().strip_prefix("gitdir:")?.trim());
    let directory = if directory.is_absolute() {
        directory
    } else {
        path.join(directory)
    };
    directory.is_dir().then_some(directory)
}

fn git_output(path: &Path, arguments: &[&OsStr]) -> Option<Vec<u8>> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt as _;
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::annotate_directory;
    use crate::models::{FileItem, GitStatus};
    use std::{fs, path::Path, process::Command};

    #[test]
    fn annotates_tracked_files_and_aggregates_folder_changes() {
        if !Path::new("/usr/bin/git").exists() {
            return;
        }
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("repository");
        fs::create_dir(&root).expect("repository directory");
        run_git(&root, &["init", "--quiet"]);
        run_git(&root, &["config", "user.name", "FlowFile Test"]);
        run_git(&root, &["config", "user.email", "flowfile@example.invalid"]);
        fs::write(root.join(".gitignore"), "ignored.txt\nignored-output/\n").expect("gitignore");
        fs::write(root.join("clean.txt"), "clean").expect("clean file");
        fs::create_dir(root.join("dist")).expect("tracked dist folder");
        fs::write(root.join("dist/tracked.js"), "tracked").expect("tracked dist file");
        fs::create_dir(root.join("folder")).expect("tracked folder");
        fs::write(root.join("folder/tracked.txt"), "before").expect("tracked nested file");
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "--quiet", "-m", "initial"]);

        let mut items = read_items(&root);
        annotate_directory(&root, &mut items);
        assert_eq!(status(&items, "clean.txt"), Some(GitStatus::Clean));
        assert_eq!(status(&items, "dist"), Some(GitStatus::Clean));
        assert_eq!(status(&items, "folder"), Some(GitStatus::Clean));

        fs::write(root.join("ignored.txt"), "ignored").expect("ignored file");
        fs::create_dir(root.join("ignored-output")).expect("ignored output folder");
        fs::write(root.join("ignored-output/cache.bin"), "ignored").expect("ignored output file");
        fs::create_dir(root.join("empty-untracked")).expect("empty untracked folder");
        fs::create_dir(root.join("target")).expect("untracked target folder");
        fs::write(root.join("target/artifact.bin"), "untracked").expect("untracked target file");
        let mut items = read_items(&root);
        annotate_directory(&root, &mut items);
        assert_eq!(status(&items, "ignored.txt"), None);
        assert_eq!(status(&items, "ignored-output"), None);
        assert_eq!(status(&items, "empty-untracked"), None);
        assert_eq!(status(&items, "target"), Some(GitStatus::Modified));

        fs::write(root.join("folder/tracked.txt"), "after").expect("modify nested file");
        fs::write(root.join("new.txt"), "new").expect("untracked file");
        let mut items = read_items(&root);
        annotate_directory(&root, &mut items);
        assert_eq!(status(&items, "clean.txt"), Some(GitStatus::Clean));
        assert_eq!(status(&items, "folder"), Some(GitStatus::Modified));
        assert_eq!(status(&items, "new.txt"), Some(GitStatus::Modified));

        let mut parent_items = read_items(directory.path());
        annotate_directory(directory.path(), &mut parent_items);
        assert_eq!(
            status(&parent_items, "repository"),
            Some(GitStatus::Modified)
        );
    }

    fn read_items(path: &Path) -> Vec<FileItem> {
        fs::read_dir(path)
            .expect("read directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != ".git")
            .map(|entry| {
                let path = entry.path();
                FileItem::from_metadata(
                    path,
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.metadata().expect("metadata"),
                    false,
                )
            })
            .collect()
    }

    fn status(items: &[FileItem], name: &str) -> Option<GitStatus> {
        items
            .iter()
            .find(|item| item.name == name)
            .and_then(|item| item.git_status)
    }

    fn run_git(path: &Path, arguments: &[&str]) {
        let status = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(path)
            .args(arguments)
            .status()
            .expect("run git");
        assert!(status.success(), "git {arguments:?} failed");
    }
}
