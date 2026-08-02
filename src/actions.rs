use crate::models::AppPreferences;
use gpui::{App, KeyBinding, actions};

actions!(
    flowfile,
    [
        CopyFiles,
        CutFiles,
        PasteFiles,
        MoveToTrash,
        PermanentDelete,
        NewFolder,
        NewTextFile,
        Duplicate,
        Refresh,
        RenameSelected,
        LayoutSingle,
        LayoutDualVertical,
        LayoutDualHorizontal,
        LayoutQuad,
        NextPane,
        PreviousPane,
        ViewDetails,
        ViewGrid,
        ToggleQuickLook,
        CloseQuickLook,
        CloseContextMenu,
        GetInfo,
        FindFiles,
        OpenTerminal,
        OpenPreferences,
        Quit,
    ]
);

pub fn register_keybindings(cx: &mut App) {
    register_keybindings_with_preferences(cx, &AppPreferences::load());
}

pub fn register_keybindings_with_preferences(cx: &mut App, preferences: &AppPreferences) {
    cx.clear_key_bindings();
    cx.bind_keys([
        KeyBinding::new("cmd-c", CopyFiles, Some("Workspace")),
        KeyBinding::new("cmd-x", CutFiles, Some("Workspace")),
        KeyBinding::new("cmd-v", PasteFiles, Some("Workspace")),
        KeyBinding::new("cmd-backspace", MoveToTrash, Some("Workspace")),
        KeyBinding::new("cmd-delete", MoveToTrash, Some("Workspace")),
        KeyBinding::new("alt-cmd-backspace", PermanentDelete, Some("Workspace")),
        KeyBinding::new("alt-cmd-delete", PermanentDelete, Some("Workspace")),
        KeyBinding::new("cmd-n", NewFolder, Some("Workspace")),
        KeyBinding::new("cmd-shift-n", NewTextFile, Some("Workspace")),
        KeyBinding::new("cmd-d", Duplicate, Some("Workspace")),
        KeyBinding::new("cmd-r", Refresh, Some("Workspace")),
        KeyBinding::new("f5", Refresh, Some("Workspace")),
        KeyBinding::new("f2", RenameSelected, Some("FileList")),
        KeyBinding::new("cmd-1", LayoutSingle, Some("Workspace")),
        KeyBinding::new("cmd-2", LayoutDualVertical, Some("Workspace")),
        KeyBinding::new("cmd-3", LayoutDualHorizontal, Some("Workspace")),
        KeyBinding::new("cmd-4", LayoutQuad, Some("Workspace")),
        KeyBinding::new("tab", NextPane, Some("Workspace")),
        KeyBinding::new("shift-tab", PreviousPane, Some("Workspace")),
        KeyBinding::new("cmd-alt-1", ViewDetails, Some("Workspace")),
        KeyBinding::new("cmd-alt-2", ViewGrid, Some("Workspace")),
        KeyBinding::new(
            &preferences.quick_look_shortcut,
            ToggleQuickLook,
            Some("FileList"),
        ),
        KeyBinding::new(
            &preferences.quick_look_shortcut,
            ToggleQuickLook,
            Some("QuickLook"),
        ),
        KeyBinding::new("escape", CloseQuickLook, Some("QuickLook")),
        KeyBinding::new("escape", CloseContextMenu, Some("FileList")),
        KeyBinding::new("escape", CloseContextMenu, Some("Workspace")),
        KeyBinding::new("cmd-i", GetInfo, Some("Workspace")),
        KeyBinding::new(&preferences.search_shortcut, FindFiles, Some("Workspace")),
        KeyBinding::new(
            &preferences.terminal_shortcut,
            OpenTerminal,
            Some("Workspace"),
        ),
        KeyBinding::new("cmd-shift-t", OpenTerminal, Some("Workspace")),
        KeyBinding::new("cmd-,", OpenPreferences, Some("Workspace")),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}
