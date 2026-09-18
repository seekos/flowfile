use super::{home_directory, persistence};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    paths: Vec<PathBuf>,
}

impl Favorites {
    pub fn load() -> Self {
        persistence::load_json_or_default(favorites_path(), "收藏夹")
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|favorite| favorite == path)
    }

    pub fn ensure_present(&mut self, path: PathBuf) -> Result<bool> {
        if self.contains(&path) {
            return Ok(false);
        }
        self.paths.push(path);
        if let Err(error) = self.save() {
            self.paths.pop();
            return Err(error);
        }
        Ok(true)
    }

    pub fn remove(&mut self, path: &Path) -> Result<bool> {
        let Some(index) = self.paths.iter().position(|favorite| favorite == path) else {
            return Ok(false);
        };

        let removed = self.paths.remove(index);
        if let Err(error) = self.save() {
            self.paths.insert(index, removed);
            return Err(error);
        }
        Ok(true)
    }

    pub fn toggle(&mut self, path: PathBuf) -> Result<bool> {
        if self.contains(&path) {
            self.remove(&path)?;
            Ok(false)
        } else {
            self.ensure_present(path)?;
            Ok(true)
        }
    }

    fn save(&self) -> Result<()> {
        let path = favorites_path();
        persistence::atomic_write_json(&path, self, "收藏夹")
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
