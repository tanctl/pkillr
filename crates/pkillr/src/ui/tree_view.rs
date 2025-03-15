use std::cmp::min;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, TreeKillPrompt, TreeRow};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let palette = app.theme().palette();

    let total_rows = app.tree_rows().len();
    let mut offset = app.tree_scroll_offset();
    let visible_height = area.height.saturating_sub(2) as usize;
    let selected_index = app.tree_selected_index().min(total_rows.saturating_sub(1));

    if visible_height > 0 && total_rows > 0 {
        if selected_index >= offset + visible_height {
            offset = selected_index + 1 - visible_height;
        } else if selected_index < offset {
            offset = selected_index;
        }
    } else {
        offset = 0;
    }
    app.set_tree_scroll_offset(offset);

    let rows = app.tree_rows();
    let end = min(offset.saturating_add(visible_height), rows.len());
    let displayed = if offset >= end {
        &rows[0..0]
    } else {
        &rows[offset..end]
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.table_border))
        .title(Line::from(vec![Span::styled(
            " Process Tree ",
            Style::default()
                .fg(palette.table_header)
                .add_modifier(Modifier::BOLD),
        )]));

    let subtree_end = subtree_range_end(rows, selected_index);

    let mut lines: Vec<Line> = if rows.is_empty() {
        vec![Line::from("No process data available."), Line::from(" ")]
    } else {
        displayed
            .iter()
            .enumerate()
            .map(|(idx, row)| {
                let global_index = offset + idx;
                let is_selected = global_index == selected_index;
                let in_subtree = global_index > selected_index && global_index < subtree_end;
                build_tree_line(app, row, is_selected, in_subtree)
            })
            .collect()
    };

    if lines.is_empty() {
        lines.push(Line::default());
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);

    if rows.len() > visible_height && visible_height > 0 {
        render_scrollbar(
            frame,
            area,
            offset,
            visible_height,
            rows.len(),
            palette.table_border,
        );
    }

    if let Some(prompt) = app.tree_kill_prompt() {
        render_kill_prompt(frame, area, palette, prompt);
    }
}

fn subtree_range_end(rows: &[TreeRow], selected_index: usize) -> usize {
    if rows.is_empty() || selected_index >= rows.len() {
        return 0;
    }
    let selected_depth = rows[selected_index].depth;
    let mut idx = selected_index + 1;
    while idx < rows.len() && rows[idx].depth > selected_depth {
        idx += 1;
    }
    idx
}

fn build_tree_line(app: &App, row: &TreeRow, is_selected: bool, in_subtree: bool) -> Line<'static> {
    let palette = app.theme().palette();
    let mut name = format!("{}{}", row.prefix, row.name);
    if row.has_children {
        name.push(' ');
        name.push_str(if row.collapsed { "[+]" } else { "[-]" });
    }

    let mut spans = Vec::new();
    spans.push(Span::styled(name, Style::default().fg(palette.text_normal)));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("(PID {})", row.pid),
        Style::default().fg(palette.text_dim),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("[CPU: {:>5.1}%]", row.subtree_cpu),
        Style::default().fg(app.theme().get_cpu_color(row.subtree_cpu)),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("[MEM: {}]", format_bytes(row.subtree_memory_bytes)),
        Style::default().fg(app.theme().get_memory_color(row.subtree_memory_bytes)),
    ));

    let mut line = Line::from(spans);
    if is_selected {
        line.style = Style::default().bg(palette.highlight_selected);
    } else if in_subtree {
        line.style = Style::default().fg(palette.text_dim);
    }
    line
}

fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    offset: usize,
    window: usize,
    total: usize,
    color: Color,
) {
    let scrollbar_area = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y + 1,
        width: 1,
        height: area.height.saturating_sub(2),
    };

    if scrollbar_area.height == 0 || window >= total {
        return;
    }

    let ratio = window as f32 / total as f32;
    let handle_height = (scrollbar_area.height as f32 * ratio).round().max(1.0) as u16;
    let max_offset = total.saturating_sub(window);
    let handle_offset = if max_offset == 0 {
        0
    } else {
        ((offset as f32 / max_offset as f32) * (scrollbar_area.height - handle_height) as f32)
            .round() as u16
    };

    let mut lines = Vec::new();
    for y in 0..scrollbar_area.height {
        let symbol = if y >= handle_offset && y < handle_offset + handle_height {
            "█"
        } else {
            "░"
        };
        lines.push(Line::from(Span::styled(
            symbol.to_string(),
            Style::default().fg(color),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, scrollbar_area);
}

fn render_kill_prompt(
    frame: &mut Frame,
    area: Rect,
    palette: crate::config::Palette,
    prompt: &TreeKillPrompt,
) {
    let mut content: Vec<Line> = Vec::new();
    let count = prompt.lines.len();
    content.push(Line::from(format!("Killing PID {} will", prompt.pid)));
    content.push(Line::from(format!("terminate {} process(es):", count)));
    content.push(Line::default());
    for line in &prompt.lines {
        content.push(Line::from(line.clone()));
    }
    content.push(Line::default());
    content.push(Line::from("Send SIGTERM? (y/n)"));

    let max_width = content
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or(20)
        .max(24);
    let popup_width = min(max_width + 4, area.width as usize) as u16;
    let popup_height = min(content.len() + 4, area.height as usize) as u16;

    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.kill_accent))
        .title(Line::from(vec![Span::styled(
            " Kill Process Tree? ",
            Style::default()
                .fg(palette.kill_accent)
                .add_modifier(Modifier::BOLD),
        )]));

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}
