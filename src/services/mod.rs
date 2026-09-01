#[cfg(target_os = "macos")]
mod external_drag;
pub mod file_engine;
mod file_inspector;
mod file_operations;
mod file_watcher;
mod quick_look;
mod search_engine;
mod smb;
mod terminal_session;
mod thumbnail_engine;
mod update_checker;
mod volume;

#[cfg(target_os = "macos")]
pub use external_drag::{begin_external_file_drag, end_external_file_drag};
pub use file_engine::{DirectorySnapshot, FileEngine, OpenWithApplication};
pub use file_inspector::FileInspector;
pub use file_operations::{ConflictPolicy, FileOperationEngine, TransferMode, TransferProgress};
pub use file_watcher::FileWatcher;
pub use quick_look::{PreviewKind, QuickLookService};
pub use search_engine::{SearchEngine, SearchScope};
pub(crate) use smb::{SmbNavigation, SmbShare};
pub use terminal_session::SystemTerminal;
pub use thumbnail_engine::ThumbnailEngine;
pub use update_checker::{AvailableUpdate, UpdateChecker};
pub use volume::{VolumeInfo, ensure_writable};
