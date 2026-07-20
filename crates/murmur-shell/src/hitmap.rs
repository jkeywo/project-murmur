//! Where the last drawn frame put things, for mouse hit-testing.
//!
//! One mechanism for every screen: a renderer records each clickable
//! span with the [`ShellInput`] it stands for, and a click resolves to
//! exactly that input before dispatch — so clicking a row *is* pressing
//! its key, on the hub, in the footers, and on the mission palette
//! alike. The mission adds a map viewport on top so clicks can also
//! resolve to tiles. Rebuilt on every draw.

use murmur_core::geom::Pos;
use ratatui::layout::Rect;

use crate::ShellInput;

/// Clickable rows recorded by a screen. Clicking a row is exactly the
/// input it carries, so every prompt on every screen — keys, Enter,
/// Esc — is reachable with the mouse.
#[derive(Clone, Debug, Default)]
pub struct ScreenLayout {
    pub actions: Vec<(u16, u16, u16, ShellInput)>,
}

impl ScreenLayout {
    /// Records a clickable span on `row` from `x0` to `x1` inclusive.
    pub fn push(&mut self, row: u16, x0: u16, x1: u16, input: ShellInput) {
        self.actions.push((row, x0, x1, input));
    }

    /// Records a whole-width row: forgiving targets for centred prompts.
    pub fn push_row(&mut self, area: Rect, row: u16, input: ShellInput) {
        self.push(row, area.x, area.x + area.width.saturating_sub(1), input);
    }

    /// The input recorded under a terminal cell, if any.
    pub(crate) fn input_at(&self, column: u16, row: u16) -> Option<ShellInput> {
        self.actions
            .iter()
            .find(|(r, x0, x1, _)| *r == row && column >= *x0 && column <= *x1)
            .map(|(_, _, _, input)| *input)
    }
}

/// The mission frame's layout: the shared clickable rows plus the map
/// viewport, so a click can resolve to a tile as well as an input.
#[derive(Clone, Debug, Default)]
pub struct UiLayout {
    /// Interior of the map viewport in terminal cells (borders excluded).
    pub map_x: u16,
    pub map_y: u16,
    pub map_w: u16,
    pub map_h: u16,
    /// The map tile rendered at the viewport's top-left interior cell.
    pub origin: Option<Pos>,
    /// Clickable rows — the same mechanism every interface screen uses.
    pub rows: ScreenLayout,
}

impl UiLayout {
    /// The map tile under a terminal cell, if any.
    pub fn tile_at(&self, column: u16, row: u16) -> Option<Pos> {
        let origin = self.origin?;
        if column < self.map_x
            || row < self.map_y
            || column >= self.map_x + self.map_w
            || row >= self.map_y + self.map_h
        {
            return None;
        }
        Some(Pos::new(
            origin.floor,
            origin.x + (column - self.map_x) as i16,
            origin.y + (row - self.map_y) as i16,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_resolve_inclusively_and_first_match_wins() {
        let mut layout = ScreenLayout::default();
        layout.push(3, 2, 6, ShellInput::Char('a'));
        layout.push(3, 2, 9, ShellInput::Char('b'));
        assert_eq!(layout.input_at(2, 3), Some(ShellInput::Char('a')));
        assert_eq!(layout.input_at(6, 3), Some(ShellInput::Char('a')));
        assert_eq!(layout.input_at(7, 3), Some(ShellInput::Char('b')));
        assert_eq!(layout.input_at(2, 4), None);
        assert_eq!(layout.input_at(10, 3), None);
    }

    #[test]
    fn whole_width_rows_cover_the_area() {
        let mut layout = ScreenLayout::default();
        let area = Rect {
            x: 5,
            y: 0,
            width: 10,
            height: 4,
        };
        layout.push_row(area, 2, ShellInput::Enter);
        assert_eq!(layout.input_at(5, 2), Some(ShellInput::Enter));
        assert_eq!(layout.input_at(14, 2), Some(ShellInput::Enter));
        assert_eq!(layout.input_at(15, 2), None);
    }

    #[test]
    fn tiles_resolve_only_inside_the_viewport() {
        let ui = UiLayout {
            map_x: 1,
            map_y: 1,
            map_w: 10,
            map_h: 5,
            origin: Some(Pos::new(0, 20, 30)),
            rows: ScreenLayout::default(),
        };
        assert_eq!(ui.tile_at(1, 1), Some(Pos::new(0, 20, 30)));
        assert_eq!(ui.tile_at(10, 5), Some(Pos::new(0, 29, 34)));
        assert_eq!(ui.tile_at(0, 1), None, "left of the viewport");
        assert_eq!(ui.tile_at(11, 1), None, "right of the viewport");
        assert_eq!(ui.tile_at(1, 6), None, "below the viewport");
        assert_eq!(
            UiLayout::default().tile_at(1, 1),
            None,
            "no origin, no tiles"
        );
    }
}
