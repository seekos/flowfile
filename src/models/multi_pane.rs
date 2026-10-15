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
        self.active_pane_index = Self::normalized_active_pane_index(
            layout_mode,
            self.active_pane_index,
            self.panes.len(),
        );
    }

    pub fn normalized_active_pane_index(
        layout_mode: LayoutMode,
        active_pane_index: usize,
        pane_count: usize,
    ) -> usize {
        if pane_count == 0 {
            return 0;
        }
        if layout_mode == LayoutMode::Single {
            return active_pane_index.min(pane_count - 1);
        }
        if active_pane_index < layout_mode.pane_count().min(pane_count) {
            active_pane_index
        } else {
            0
        }
    }

    pub fn visible_pane_indices(&self) -> Vec<usize> {
        if self.panes.is_empty() {
            return Vec::new();
        }
        if self.layout_mode == LayoutMode::Single {
            return vec![self.active_pane_index.min(self.panes.len() - 1)];
        }
        (0..self.layout_mode.pane_count().min(self.panes.len())).collect()
    }

    pub fn is_pane_visible(&self, index: usize) -> bool {
        self.visible_pane_indices().contains(&index)
    }

    pub fn set_active_pane(&mut self, index: usize) {
        if self.is_pane_visible(index) {
            if self.active_pane_index != index {
                self.last_active_pane_index = Some(self.active_pane_index);
            }
            self.active_pane_index = index;
        }
    }

    pub fn cycle_active_pane(&mut self, reverse: bool) {
        let visible = self.visible_pane_indices();
        if visible.len() <= 1 {
            return;
        }
        let current = visible
            .iter()
            .position(|index| *index == self.active_pane_index)
            .unwrap_or(0);
        let next = if reverse {
            (current + visible.len() - 1) % visible.len()
        } else {
            (current + 1) % visible.len()
        };
        self.set_active_pane(visible[next]);
    }

    pub fn other_pane_index(&self) -> Option<usize> {
        let visible = self.visible_pane_indices();
        self.last_active_pane_index
            .filter(|index| visible.contains(index) && *index != self.active_pane_index)
            .or_else(|| {
                visible
                    .into_iter()
                    .find(|index| *index != self.active_pane_index)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{LayoutMode, MultiPaneModel};

    #[test]
    fn single_layout_keeps_the_selected_pane_index() {
        assert_eq!(
            MultiPaneModel::normalized_active_pane_index(LayoutMode::Single, 2, 4),
            2
        );
    }

    #[test]
    fn multi_pane_layout_falls_back_when_the_selected_pane_is_hidden() {
        assert_eq!(
            MultiPaneModel::normalized_active_pane_index(LayoutMode::DualVertical, 3, 4),
            0
        );
    }
}
