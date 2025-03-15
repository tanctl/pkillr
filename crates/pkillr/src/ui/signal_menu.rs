use std::cmp::min;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::signals::Signal;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let signals = Signal::all();
    if signals.is_empty() {
        return;
    }

    let palette = app.theme().palette();

    let dim = Block::default().style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(dim, area);

    let popup_width = (area.width as f32 * 0.5).max(30.0) as u16;
    let popup_height = (area.height as f32 * 0.7).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width.min(area.width),
        height: popup_height.min(area.height),
    };

    let inner_height = popup.height.saturating_sub(4) as usize; // borders + title + hint
    let mut offset = app.signal_menu_scroll_offset();
    let selected = app
        .signal_menu_selected()
        .min(signals.len().saturating_sub(1));

    if inner_height > 0 {
        if selected >= offset + inner_height {
            offset = selected + 1 - inner_height;
        } else if selected < offset {
            offset = selected;
        }
    } else {
        offset = 0;
    }
    offset = offset.min(signals.len().saturating_sub(1));
    app.set_signal_menu_scroll_offset(offset);

    let end = min(offset.saturating_add(inner_height), signals.len());
    let displayed = if offset >= end {
        &signals[0..0]
    } else {
        &signals[offset..end]
    };

    let items: Vec<ListItem> = displayed
        .iter()
        .map(|signal| {
            let number = format!("{:>2}", signal.number());
            let name = format!("{:<8}", signal.name());
            let description = signal.description();
            let line = Line::from(vec![
                Span::styled(number, Style::default().fg(palette.text_dim)),
                Span::raw("  "),
                Span::styled(name, Style::default().fg(palette.text_normal)),
                Span::raw("  "),
                Span::styled(description, Style::default().fg(palette.text_dim)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let highlight = Style::default()
        .bg(palette.highlight_selected)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let list = List::new(items).highlight_style(highlight);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.table_border))
        .title(Line::from(Span::styled(
            " Select Signal ",
            Style::default()
                .fg(palette.table_header)
                .add_modifier(Modifier::BOLD),
        )));

    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let margin = ratatui::layout::Margin::new(1, 1);
    let inner = popup.inner(&margin);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let mut state = ListState::default();
    if selected < signals.len() {
        state.select(Some(selected - offset));
    }
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let hints = Paragraph::new("↑↓/jk navigate | Enter send | 1-9 select | Esc cancel")
        .style(Style::default().fg(palette.text_dim))
        .wrap(Wrap { trim: true });
    frame.render_widget(hints, chunks[1]);
}
