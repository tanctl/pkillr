use ratatui::Frame;

use crate::app::App;

pub mod aux_views;
pub mod info_pane;
pub mod signal_menu;
pub mod table;
pub mod tree_view;

pub fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.size();
    table::render(frame, area, app);
}
