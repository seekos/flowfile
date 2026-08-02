use super::{Model, Pane};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum LayoutMode {
    #[default]
    Single,
    DualVertical,
    DualHorizontal,
    Quad,
}

impl LayoutMode {
    pub fn pane_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::DualVertical | Self::DualHorizontal => 2,
            Self::Quad => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "单面板",
            Self::DualVertical => "左右双面板",
            Self::DualHorizontal => "上下双面板",
            Self::Quad => "四面板",
        }
    }
}

pub struct MultiPaneModel {
    pub layout_mode: LayoutMode,
    pub panes: Vec<Model<Pane>>,
    pub active_pane_index: usize,
    pub last_active_pane_index: Option<usize>,
}

impl MultiPaneModel {
    pub fn set_layout(&mut self, layout_mode: LayoutMode) {
        self.layout_mode = layout_mode;
        if self.active_pane_index >= layout_mode.pane_count() {
            self.active_pane_index = 0;
        }
    }

    pub fn set_active_pane(&mut self, index: usize) {
        if index < self.layout_mode.pane_count() && index < self.panes.len() {
            if self.active_pane_index != index {
                self.last_active_pane_index = Some(self.active_pane_index);
            }
            self.active_pane_index = index;
        }
    }

    pub fn cycle_active_pane(&mut self, reverse: bool) {
        let count = self.layout_mode.pane_count().min(self.panes.len());
        if count <= 1 {
            return;
        }
        let next = if reverse {
            (self.active_pane_index + count - 1) % count
        } else {
            (self.active_pane_index + 1) % count
        };
        self.set_active_pane(next);
    }

    pub fn other_pane_index(&self) -> Option<usize> {
        let count = self.layout_mode.pane_count().min(self.panes.len());
        self.last_active_pane_index
            .filter(|index| *index < count && *index != self.active_pane_index)
            .or_else(|| (0..count).find(|index| *index != self.active_pane_index))
    }
}
