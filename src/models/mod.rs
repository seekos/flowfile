mod favorites;
mod file_item;
mod multi_pane;
mod operations;
pub mod pane;
mod preferences;
mod session;

pub use favorites::Favorites;
pub use file_item::{FileItem, FileKind, SortMode};
pub use multi_pane::{LayoutMode, MultiPaneModel};
pub use operations::{CreateItemKind, FileDragPayload, FileOperationController};
pub use pane::{ExplorerTab, Pane, ViewMode, home_directory};
pub use preferences::{AppPreferences, ThemePreference};
pub use session::SessionState;

/// GPUI 0.2 calls retained state handles `Entity<T>`. Keeping this alias makes
/// the application model read naturally as a collection of pane models.
pub type Model<T> = gpui::Entity<T>;
