use anyhow::Result;
use gpui::{AssetSource, Hsla, SharedString, Svg, prelude::*, px, svg};
use std::borrow::Cow;

/// Monochrome toolbar symbols drawn in the same compact, rounded style as
/// macOS template images. GPUI applies the requested text color to the SVG
/// alpha mask, so every icon automatically follows light/dark and active
/// states without shipping separate artwork.
#[derive(Clone, Copy)]
pub enum IconName {
    Sidebar,
    LayoutSingle,
    LayoutColumns,
    LayoutRows,
    LayoutGrid,
    Eye,
    EyeSlash,
    CopyRight,
    MoveRight,
    Trash,
    FolderAdd,
    Settings,
    ChevronLeft,
    ChevronRight,
    ArrowUp,
    Edit,
    Refresh,
    Search,
    Close,
}

impl IconName {
    fn path(self) -> &'static str {
        match self {
            Self::Sidebar => "icons/sidebar.svg",
            Self::LayoutSingle => "icons/layout-single.svg",
            Self::LayoutColumns => "icons/layout-columns.svg",
            Self::LayoutRows => "icons/layout-rows.svg",
            Self::LayoutGrid => "icons/layout-grid.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeSlash => "icons/eye-slash.svg",
            Self::CopyRight => "icons/copy-right.svg",
            Self::MoveRight => "icons/move-right.svg",
            Self::Trash => "icons/trash.svg",
            Self::FolderAdd => "icons/folder-add.svg",
            Self::Settings => "icons/settings.svg",
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Edit => "icons/edit.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Search => "icons/search.svg",
            Self::Close => "icons/close.svg",
        }
    }
}

pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg().path(name.path()).size(px(size)).text_color(color)
}

pub struct IconAssets;

impl AssetSource for IconAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/sidebar.svg" => Some(SIDEBAR),
            "icons/layout-single.svg" => Some(LAYOUT_SINGLE),
            "icons/layout-columns.svg" => Some(LAYOUT_COLUMNS),
            "icons/layout-rows.svg" => Some(LAYOUT_ROWS),
            "icons/layout-grid.svg" => Some(LAYOUT_GRID),
            "icons/eye.svg" => Some(EYE),
            "icons/eye-slash.svg" => Some(EYE_SLASH),
            "icons/copy-right.svg" => Some(COPY_RIGHT),
            "icons/move-right.svg" => Some(MOVE_RIGHT),
            "icons/trash.svg" => Some(TRASH),
            "icons/folder-add.svg" => Some(FOLDER_ADD),
            "icons/settings.svg" => Some(SETTINGS),
            "icons/chevron-left.svg" => Some(CHEVRON_LEFT),
            "icons/chevron-right.svg" => Some(CHEVRON_RIGHT),
            "icons/arrow-up.svg" => Some(ARROW_UP),
            "icons/edit.svg" => Some(EDIT),
            "icons/refresh.svg" => Some(REFRESH),
            "icons/search.svg" => Some(SEARCH),
            "icons/close.svg" => Some(CLOSE),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path == "icons" {
            Ok(vec![
                "sidebar.svg".into(),
                "layout-single.svg".into(),
                "layout-columns.svg".into(),
                "layout-rows.svg".into(),
                "layout-grid.svg".into(),
                "eye.svg".into(),
                "eye-slash.svg".into(),
                "copy-right.svg".into(),
                "move-right.svg".into(),
                "trash.svg".into(),
                "folder-add.svg".into(),
                "settings.svg".into(),
                "chevron-left.svg".into(),
                "chevron-right.svg".into(),
                "arrow-up.svg".into(),
                "edit.svg".into(),
                "refresh.svg".into(),
                "search.svg".into(),
                "close.svg".into(),
            ])
        } else {
            Ok(Vec::new())
        }
    }
}

const SIDEBAR: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="2.5" y="3" width="15" height="14" rx="2.2" fill="none" stroke="#000" stroke-width="1.7"/><path d="M7 3.4v13.2" fill="none" stroke="#000" stroke-width="1.7"/><circle cx="4.75" cy="6" r=".8" fill="#000"/><circle cx="4.75" cy="9" r=".8" fill="#000"/></svg>"##;
const LAYOUT_SINGLE: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="14" height="14" rx="2.1" fill="none" stroke="#000" stroke-width="1.7"/></svg>"##;
const LAYOUT_COLUMNS: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="2.5" y="3" width="15" height="14" rx="2.1" fill="none" stroke="#000" stroke-width="1.7"/><path d="M10 3.6v12.8" fill="none" stroke="#000" stroke-width="1.7"/></svg>"##;
const LAYOUT_ROWS: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="2.5" width="14" height="15" rx="2.1" fill="none" stroke="#000" stroke-width="1.7"/><path d="M3.6 10h12.8" fill="none" stroke="#000" stroke-width="1.7"/></svg>"##;
const LAYOUT_GRID: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="2.5" y="2.5" width="15" height="15" rx="2.1" fill="none" stroke="#000" stroke-width="1.7"/><path d="M10 3.1v13.8M3.1 10h13.8" fill="none" stroke="#000" stroke-width="1.7"/></svg>"##;
const EYE: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M2.2 10s2.8-5 7.8-5 7.8 5 7.8 5-2.8 5-7.8 5-7.8-5-7.8-5Z" fill="none" stroke="#000" stroke-width="1.7" stroke-linejoin="round"/><circle cx="10" cy="10" r="2.2" fill="none" stroke="#000" stroke-width="1.7"/></svg>"##;
const EYE_SLASH: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M3.1 7.3C2.5 8 2.2 8.7 2.2 8.7S5 13.7 10 13.7c1.2 0 2.3-.3 3.2-.8M16.8 11.4c.7-.8 1-1.5 1-1.5s-2.8-5-7.8-5c-1.1 0-2 .2-2.9.6M4 3.3 16 16.7" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const COPY_RIGHT: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><rect x="2.5" y="5" width="8.5" height="10" rx="1.7" fill="none" stroke="#000" stroke-width="1.6"/><path d="M6 5V3.8C6 2.8 6.8 2 7.8 2h7.7c1.1 0 2 .9 2 2v8c0 1.1-.9 2-2 2H14M12.5 7.5 15 10l-2.5 2.5M15 10H9" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const MOVE_RIGHT: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M2.5 5.2V3.8c0-1 .8-1.8 1.8-1.8h7.4c1 0 1.8.8 1.8 1.8v1.4M5.5 9H16M12.8 5.8 16 9l-3.2 3.2M13.5 12.8v3.4c0 1-.8 1.8-1.8 1.8H4.3c-1 0-1.8-.8-1.8-1.8v-7" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const TRASH: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M3.2 5.5h13.6M7.2 3h5.6l.7 2.5M5 5.5l.7 11.2h8.6L15 5.5M8 8v5.8M12 8v5.8" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const FOLDER_ADD: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M2 5.3c0-1.1.9-2 2-2h3l1.7 2H16c1.1 0 2 .9 2 2v7.3c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V5.3Z" fill="none" stroke="#000" stroke-width="1.6" stroke-linejoin="round"/><path d="M10 8v5M7.5 10.5h5" fill="none" stroke="#000" stroke-width="1.7" stroke-linecap="round"/></svg>"##;
const SETTINGS: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="m8.5 2.7.4-1h2.2l.4 1 .9.4 1-.4 1.5 1.5-.4 1 .4.9 1 .4v2.2l-1 .4-.4.9.4 1-1.5 1.5-1-.4-.9.4-.4 1H8.9l-.4-1-.9-.4-1 .4L5.1 11l.4-1-.4-.9-1-.4V6.5l1-.4.4-.9-.4-1 1.5-1.5 1 .4.9-.4Z" transform="translate(0 2.4)" fill="none" stroke="#000" stroke-width="1.45" stroke-linejoin="round"/><circle cx="10" cy="10" r="2.1" fill="none" stroke="#000" stroke-width="1.6"/></svg>"##;
const CHEVRON_LEFT: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="m12.5 4.2-5.8 5.8 5.8 5.8" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CHEVRON_RIGHT: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="m7.5 4.2 5.8 5.8-5.8 5.8" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ARROW_UP: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M10 17V3M4.8 8.2 10 3l5.2 5.2" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const EDIT: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="m12.2 3.5 4.3 4.3M4 16l2.2-5.2L13.9 3c.8-.8 2-.8 2.8 0l.3.3c.8.8.8 2 0 2.8l-7.8 7.7L4 16Z" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const REFRESH: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="M16.4 7.5A6.7 6.7 0 1 0 16 13M16.4 3.7v3.8h-3.8" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const SEARCH: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><circle cx="8.7" cy="8.7" r="5.7" fill="none" stroke="#000" stroke-width="1.7"/><path d="m13 13 4 4" fill="none" stroke="#000" stroke-width="1.9" stroke-linecap="round"/></svg>"##;
const CLOSE: &[u8] = br##"<svg viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path d="m5 5 10 10M15 5 5 15" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
