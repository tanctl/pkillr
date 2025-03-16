use chrono::Local;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;

pub fn render_signal_history(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(60, 70, area);
    let palette = app.theme().palette();

    let mut lines = Vec::new();
    let history = app.signal_history();

    if history.is_empty() {
        lines.push(Line::from("no signals sent yet."));
    } else {
        for (idx, entry) in history.iter().enumerate() {
            let ts = entry
                .timestamp
                .with_timezone(&Local)
                .format("%H:%M:%S")
                .to_string();
            let header = format!("{}  {} ({})", ts, entry.process_name, entry.pid);
            lines.push(Line::from(Span::styled(
                header,
                Style::default()
                    .fg(palette.text_normal)
                    .add_modifier(Modifier::BOLD),
            )));

            let status_color = if entry.result.is_ok() {
                Color::Green
            } else {
                palette.status_error
            };
            let status_text = match &entry.result {
                Ok(_) => "Success".to_string(),
                Err(err) => app.friendly_error_message(err),
            };

            lines.push(Line::from(vec![
                Span::raw("           "),
                Span::styled(
                    entry.signal.name(),
                    Style::default()
                        .fg(palette.text_normal)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" \u{2192} "),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]));

            if idx + 1 < history.len() {
                lines.push(Line::default());
            }
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.table_border))
        .title(Line::from(Span::styled(
            " Signal History ",
            Style::default()
                .fg(palette.table_header)
                .add_modifier(Modifier::BOLD),
        )));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

pub fn render_help_popup(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(70, 80, area);
    let palette = app.theme().palette();

    let heading = Style::default()
        .fg(palette.table_header)
        .add_modifier(Modifier::BOLD);
    let body = Style::default().fg(palette.text_normal);
    let dim = Style::default().fg(palette.text_dim);

    let mut lines = Vec::new();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("NAVIGATION", heading)));
    lines.push(Line::from(Span::styled("  ↑↓ / j k  move selection", body)));
    lines.push(Line::from(Span::styled(
        "  g G       jump top/bottom",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  < >       cycle sort column",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  Esc       close info/tree",
        body,
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("ACTIONS", heading)));
    lines.push(Line::from(Span::styled("  /         fuzzy search", body)));
    lines.push(Line::from(Span::styled("  /^...$/  regex filter", body)));
    lines.push(Line::from(Span::styled("  /killed  history filter", body)));
    lines.push(Line::from(Span::styled(
        "  Space     select / toggle",
        body,
    )));
    lines.push(Line::from(Span::styled("  Enter/k   kill (SIGTERM)", body)));
    lines.push(Line::from(Span::styled(
        "  K         force kill (SIGKILL)",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  x         kill tree (preview)",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  s         open signal menu",
        body,
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("VIEWS", heading)));
    lines.push(Line::from(Span::styled(
        "  i         toggle info pane",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  Tab       switch info focus",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  e/f/m/n/c toggle info sections",
        body,
    )));
    lines.push(Line::from(Span::styled(
        "  t         toggle process tree",
        body,
    )));
    lines.push(Line::from(Span::styled("  h         signal history", body)));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("  ?         this help", body)));
    lines.push(Line::from(Span::styled("  q         quit", body)));
    lines.push(Line::from(Span::styled("  Ctrl+C    quit instantly", body)));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Press any key to close",
        dim.add_modifier(Modifier::ITALIC),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.table_border))
        .title(Line::from(Span::styled(
            " pkillr Help ",
            Style::default()
                .fg(palette.table_header)
                .add_modifier(Modifier::BOLD),
        )));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_width = (area.width * percent_x) / 100;
    let popup_height = (area.height * percent_y) / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width.max(1),
        height: popup_height.max(1),
    }
}
