use std::cmp::{max, min};
use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::app::{App, AppMode, StatusLevel};
use crate::process::{self, ProcessInfo};
use crate::ui::{aux_views, info_pane, signal_menu, tree_view};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);

    render_header(frame, layout[0], app);
    if app.tree_view_open() {
        tree_view::render(frame, layout[1], app);
    } else {
        render_table(frame, layout[1], app);
    }
    render_status(frame, layout[2], app);

    if app.signal_menu_open() {
        signal_menu::render(frame, area, app);
    }
    if app.history_popup_open() {
        aux_views::render_signal_history(frame, area, app);
    }
    if app.help_popup_open() {
        aux_views::render_help_popup(frame, area, app);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.theme().palette();
    let mode_text = if app.is_info_pane_open() && matches!(app.mode(), AppMode::Normal) {
        "INFO"
    } else {
        mode_label(app.mode())
    };
    let mut spans = vec![
        Span::styled("pkillr", Style::default().fg(palette.table_header)),
        Span::raw(" | "),
        Span::styled(mode_text, Style::default().fg(palette.text_normal)),
        Span::raw(" | "),
        Span::styled(
            format!("{} processes", app.filtered_processes().len()),
            Style::default().fg(palette.text_dim),
        ),
    ];

    if !app.search_query().is_empty() {
        spans.push(Span::raw(" | filter: "));
        spans.push(Span::styled(
            app.search_query(),
            Style::default().fg(palette.kill_accent),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans)).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn render_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut table_area = area;
    let mut info_area = None;

    if app.is_info_pane_open() {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        table_area = chunks[0];
        info_area = Some(chunks[1]);
    }

    render_process_list(frame, table_area, app);

    if let Some(info_rect) = info_area {
        info_pane::render(frame, info_rect, app);
    }
}

fn render_process_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let palette = app.theme().palette();
    let row_count = {
        let processes = app.filtered_processes();
        processes.len()
    };
    let visible_height = area.height.saturating_sub(3) as usize; // borders + header
    let selected_index = app.selected_index();

    let mut offset = app.table_scroll_offset();
    if visible_height > 0 {
        if selected_index >= offset + visible_height {
            offset = selected_index + 1 - visible_height;
        } else if selected_index < offset {
            offset = selected_index;
        }
    } else {
        offset = 0;
    }
    app.set_table_scroll_offset(offset);

    let processes = app.filtered_processes();
    if row_count == 0 {
        let message = if app.search_query().trim().is_empty() {
            "No processes found".to_string()
        } else {
            format!("No matches for '{}'", app.search_query())
        };
        let paragraph = Paragraph::new(Line::from(Span::styled(
            message,
            Style::default().fg(palette.text_dim),
        )))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.table_border)),
        );
        frame.render_widget(paragraph, area);
        return;
    }

    let end = min(offset.saturating_add(visible_height), row_count);
    let displayed = if offset >= end {
        &processes[0..0]
    } else {
        &processes[offset..end]
    };

    let header_cells = ["PID", "Name", "CPU%", "MEM%", "User", "Runtime"]
        .into_iter()
        .map(|title| Cell::from(title).style(Style::default().fg(palette.table_header)));

    let header = Row::new(header_cells).height(1);

    let rows = displayed.iter().enumerate().map(|(idx, proc)| {
        let absolute_index = idx + offset;
        build_row(app, proc, absolute_index == selected_index)
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.table_border)),
        )
        .header(header)
        .column_spacing(1);

    frame.render_widget(table, area);

    if row_count > visible_height && visible_height > 0 {
        render_scrollbar(
            frame,
            area,
            offset,
            visible_height,
            row_count,
            palette.table_border,
        );
    }
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.theme().palette();
    let mut lines = vec![Line::from(""), Line::from("")];

    if let Some((message, level)) = app.status_message() {
        let color = match level {
            StatusLevel::Info => palette.status_info,
            StatusLevel::Warning => palette.status_warning,
            StatusLevel::Error => palette.status_error,
        };
        lines[0] = Line::from(Span::styled(message.clone(), Style::default().fg(color)));
    }

    lines[1] = Line::from(Span::styled(
        hints_for_mode(app),
        Style::default().fg(palette.text_dim),
    ));

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(palette.table_border));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn build_row(app: &App, proc: &ProcessInfo, is_selected: bool) -> Row<'static> {
    let palette = app.theme().palette();
    let mut style = app.theme().style_for_process(proc);
    let needs_sudo = !app.can_kill_without_privileges(proc);

    if needs_sudo {
        style = style
            .fg(palette.text_dim)
            .add_modifier(Modifier::DIM)
            .add_modifier(Modifier::ITALIC);
    }

    if is_selected {
        style = style.bg(palette.highlight_selected);
    }

    let pid = format!("{:>8}", proc.pid);
    let highlight_bytes = app.highlight_indices(proc.pid).unwrap_or(&[]);
    let highlight_chars = highlight_char_positions(&proc.name, highlight_bytes);

    let mut sequence: Vec<(char, bool)> = Vec::new();
    if app.is_pid_selected(proc.pid) {
        sequence.push(('✓', false));
        sequence.push((' ', false));
    }
    for (idx, ch) in proc.name.chars().enumerate() {
        let highlight = highlight_chars.contains(&idx);
        sequence.push((ch, highlight));
    }
    if needs_sudo || process::is_system_process(proc) {
        for ch in " [needs sudo]".chars() {
            sequence.push((ch, false));
        }
    }

    let truncated_seq = truncate_sequence(&sequence, 20);
    let name_spans = sequence_to_spans(
        truncated_seq,
        Style::default().fg(palette.text_normal),
        Style::default()
            .fg(palette.kill_accent)
            .add_modifier(Modifier::BOLD),
    );
    let name_cell = Cell::from(Line::from(name_spans));

    let cpu = format!("{:>5.1}%", proc.cpu_percent);
    let mem = format!("{:>5.1}%", memory_percent(proc, app.total_memory_bytes()));
    let user = truncated(&proc.user, 12);
    let runtime = format_runtime(proc.runtime);

    let cpu_style = Style::default().fg(app.theme().get_cpu_color(proc.cpu_percent));
    let mem_style = Style::default().fg(app.theme().get_memory_color(proc.memory_bytes));

    Row::new(vec![
        Cell::from(pid),
        name_cell,
        Cell::from(cpu).style(cpu_style),
        Cell::from(mem).style(mem_style),
        Cell::from(user),
        Cell::from(runtime),
    ])
    .style(style)
    .height(1)
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

    if scrollbar_area.height == 0 {
        return;
    }

    let ratio = window as f32 / total as f32;
    let handle_height = max((scrollbar_area.height as f32 * ratio).round() as u16, 1);
    let max_offset = total.saturating_sub(window);
    let handle_offset = if max_offset == 0 {
        0
    } else {
        ((offset as f32 / max_offset as f32) * (scrollbar_area.height - handle_height) as f32)
            .round() as u16
    };

    let lines: Vec<Line> = (0..scrollbar_area.height)
        .map(|y| {
            let symbol = if y >= handle_offset && y < handle_offset + handle_height {
                "█"
            } else {
                "░"
            };
            Line::from(Span::styled(symbol.to_string(), Style::default().fg(color)))
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, scrollbar_area);
}

fn hints_for_mode(app: &App) -> String {
    match app.mode() {
        AppMode::Normal => {
            let mut parts = vec!["↑↓/jk move", "g/G top/bot", "< > sort"];

            if app.is_info_pane_open() {
                parts.push("Esc close info");
                parts.push(if app.info_focus() {
                    "Tab table"
                } else {
                    "Tab focus info"
                });
                parts.push("e/f/m/n/c toggle sections");
            } else {
                parts.push("i info");
            }

            parts.push(if app.tree_view_open() {
                "t table"
            } else {
                "t tree"
            });

            if app.has_selection() {
                parts.push("Space toggle");
                parts.push("Enter/k kill");
                parts.push("K sigkill");
                parts.push("x tree kill");
            } else {
                parts.push("Space select");
                parts.push("k kill current");
                parts.push("s signal menu");
            }

            parts.push("h history");
            parts.push("? help");
            parts.push("q quit");
            parts.join(" | ")
        }
        AppMode::Search => {
            "Type to filter | /pattern/ regex | /killed history | Enter/Esc exit".to_string()
        }
        AppMode::SignalMenu => "Esc cancel | ↑↓/jk navigate | 1-9 select | Enter send".to_string(),
        AppMode::InfoPane => {
            "Esc close info | Tab toggle focus | e/f/n/c expand sections".to_string()
        }
        AppMode::TreeView => {
            "Esc close tree | ↑↓/jk move | Space collapse | x kill tree".to_string()
        }
        AppMode::HistoryView => "Any key close history".to_string(),
    }
}

fn mode_label(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Normal => "NORMAL",
        AppMode::Search => "SEARCH",
        AppMode::SignalMenu => "SIGNAL",
        AppMode::InfoPane => "INFO",
        AppMode::TreeView => "TREE",
        AppMode::HistoryView => "HISTORY",
    }
}

fn truncated(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        value.chars().take(max_len).collect()
    }
}

fn highlight_char_positions(text: &str, byte_indices: &[usize]) -> HashSet<usize> {
    if byte_indices.is_empty() {
        return HashSet::new();
    }
    let byte_set: HashSet<usize> = byte_indices.iter().copied().collect();
    text.char_indices()
        .enumerate()
        .filter_map(|(idx, (byte_idx, _))| {
            if byte_set.contains(&byte_idx) {
                Some(idx)
            } else {
                None
            }
        })
        .collect()
}

fn truncate_sequence(seq: &[(char, bool)], max_len: usize) -> Vec<(char, bool)> {
    if seq.len() <= max_len {
        return seq.to_vec();
    }
    if max_len == 0 {
        return Vec::new();
    }
    let mut truncated = Vec::with_capacity(max_len);
    for (index, item) in seq.iter().enumerate() {
        if index >= max_len {
            break;
        }
        truncated.push(*item);
    }
    if let Some(last) = truncated.last_mut() {
        *last = ('…', false);
    }
    truncated
}

fn sequence_to_spans(
    seq: Vec<(char, bool)>,
    base_style: Style,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    if seq.is_empty() {
        return vec![Span::styled(String::new(), base_style)];
    }

    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut active: Option<bool> = None;

    for (ch, highlight) in seq {
        match active {
            Some(state) if state == highlight => buffer.push(ch),
            Some(state) => {
                let style = if state { highlight_style } else { base_style };
                spans.push(Span::styled(buffer.clone(), style));
                buffer.clear();
                buffer.push(ch);
                active = Some(highlight);
            }
            None => {
                buffer.push(ch);
                active = Some(highlight);
            }
        }
    }

    if let Some(state) = active {
        let style = if state { highlight_style } else { base_style };
        spans.push(Span::styled(buffer, style));
    }

    spans
}

fn memory_percent(proc: &ProcessInfo, total_memory_bytes: u64) -> f32 {
    if total_memory_bytes == 0 {
        return 0.0;
    }
    let ratio = proc.memory_bytes as f64 / total_memory_bytes as f64;
    (ratio * 100.0) as f32
}

fn format_runtime(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    let minutes = secs / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if days > 0 {
        format!("{}d {}h", days, hours % 24)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes % 60)
    } else {
        format!("{}m {}s", minutes, secs % 60)
    }
}
