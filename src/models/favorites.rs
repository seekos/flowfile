use super::home_directory;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    paths: Vec<PathBuf>,
}

impl Favorites {
    pub fn load() -> Self {
        fs::read(favorites_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn contains(&self, path: &std::path::Path) -> bool {
        self.paths.iter().any(|favorite| favorite == path)
    }

    pub fn toggle(&mut self, path: PathBuf) -> Result<bool> {
        let added = if let Some(index) = self.paths.iter().position(|favorite| favorite == &path) {
            self.paths.remove(index);
            false
        } else {
            self.paths.push(path);
            true
        };
        self.save()?;
        Ok(added)
    }

    fn save(&self) -> Result<()> {
        let path = favorites_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建收藏夹设置目录 {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(&path, bytes).with_context(|| format!("无法保存收藏夹 {}", path.display()))
    }
}

fn favorites_path() -> PathBuf {
    home_directory()
        .join("Library")
        .join("Application Support")
        .join("FlowFile")
        .join("favorites.json")
}

#[cfg(test)]
mod tests {
    use super::Favorites;
    use std::path::{Path, PathBuf};

    #[test]
    fn toggle_adds_and_removes_a_path() {
        let mut favorites = Favorites::default();
        let path = PathBuf::from("/tmp/example");
        assert!(favorites.toggle_in_memory(path.clone()));
        assert!(favorites.contains(Path::new("/tmp/example")));
        assert!(!favorites.toggle_in_memory(path));
        assert!(favorites.paths().is_empty());
    }

    impl Favorites {
        fn toggle_in_memory(&mut self, path: PathBuf) -> bool {
            if let Some(index) = self.paths.iter().position(|favorite| favorite == &path) {
                self.paths.remove(index);
                false
            } else {
                self.paths.push(path);
                true
            }
        }
    }
}
