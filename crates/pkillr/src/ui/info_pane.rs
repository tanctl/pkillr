use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::config::Palette;
use crate::process::{ChildProcess, ProcessDetails};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let palette = app.theme().palette();
    let border_color = if app.info_focus() {
        palette.highlight_selected
    } else {
        palette.table_border
    };

    let mut lines = Vec::new();

    let env_expanded = app.info_env_expanded();
    let files_expanded = app.info_files_expanded();
    let network_expanded = app.info_network_expanded();
    let cgroups_expanded = app.info_cgroups_expanded();

    if let Some(details) = app.process_details() {
        build_basic_section(&mut lines, &palette, details);
        build_command_section(&mut lines, &palette, details);
        build_children_section(&mut lines, &palette, details.children.as_slice());
        build_capabilities_section(&mut lines, &palette, details);
        build_environment_section(&mut lines, &palette, env_expanded, details);
        build_open_files_section(&mut lines, &palette, files_expanded, details);
        build_network_section(&mut lines, &palette, network_expanded, details);
        build_cgroup_section(&mut lines, &palette, cgroups_expanded, details);
    } else {
        lines.push(Line::from("No process selected."));
        lines.push(Line::from("Select a process to view details."));
    }

    if lines.is_empty() {
        lines.push(Line::default());
    }

    let mut title_spans = vec![Span::styled(
        " Process Details ",
        Style::default()
            .fg(palette.table_header)
            .add_modifier(Modifier::BOLD),
    )];
    if app.info_focus() {
        title_spans.push(Span::raw(" [focused]"));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_spans));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.info_pane_scroll(), 0));

    frame.render_widget(paragraph, area);
}

fn build_basic_section(lines: &mut Vec<Line>, palette: &Palette, details: &ProcessDetails) {
    let label = label_style(palette);
    let value = value_style(palette);

    push_line(
        lines,
        Line::from(vec![
            Span::styled("PID: ", label),
            Span::styled(details.pid.to_string(), value),
        ]),
    );

    push_line(
        lines,
        Line::from(vec![
            Span::styled("Parent PID: ", label),
            Span::styled(
                details
                    .parent_pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                value,
            ),
        ]),
    );

    push_line(
        lines,
        Line::from(vec![
            Span::styled("State: ", label),
            Span::styled(details.state.as_str(), value),
        ]),
    );

    push_line(
        lines,
        Line::from(vec![
            Span::styled("Threads: ", label),
            Span::styled(details.thread_count.to_string(), value),
        ]),
    );

    push_blank_line(lines);

    let cwd = details
        .cwd
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unknown>".to_string());
    push_line(
        lines,
        Line::from(vec![
            Span::styled("Working Dir: ", label),
            Span::styled(cwd, value),
        ]),
    );
}

fn build_command_section(lines: &mut Vec<Line>, palette: &Palette, details: &ProcessDetails) {
    push_blank_line(lines);
    let label = label_style(palette);
    push_line(
        lines,
        Line::from(Span::styled("Command:", label.add_modifier(Modifier::BOLD))),
    );

    let command = if details.cmdline.is_empty() {
        "<unknown>".to_string()
    } else {
        details.cmdline.join(" ")
    };
    push_line(lines, Line::from(format!("  {}", command)));
}

fn build_children_section(lines: &mut Vec<Line>, palette: &Palette, children: &[ChildProcess]) {
    push_blank_line(lines);
    let label = label_style(palette);
    push_line(
        lines,
        Line::from(Span::styled(
            "Children:",
            label.add_modifier(Modifier::BOLD),
        )),
    );

    if children.is_empty() {
        push_line(lines, Line::from("  (none)"));
        return;
    }

    for (index, child) in children.iter().enumerate() {
        let branch = if index + 1 == children.len() {
            "└─"
        } else {
            "├─"
        };
        let entry = format!(
            "  {} {} {} ({})",
            branch,
            child.pid,
            child.name,
            child.state.as_str()
        );
        push_line(lines, Line::from(entry));
    }
}

fn build_capabilities_section(lines: &mut Vec<Line>, palette: &Palette, details: &ProcessDetails) {
    push_blank_line(lines);
    let label = label_style(palette);
    push_line(
        lines,
        Line::from(Span::styled(
            "Capabilities:",
            label.add_modifier(Modifier::BOLD),
        )),
    );

    if details.capabilities.is_empty() {
        push_line(lines, Line::from("  <unavailable>"));
    } else {
        for cap in &details.capabilities {
            push_line(lines, Line::from(format!("  {}", cap)));
        }
    }
}

fn build_environment_section(
    lines: &mut Vec<Line>,
    palette: &Palette,
    expanded: bool,
    details: &ProcessDetails,
) {
    push_blank_line(lines);
    let label = label_style(palette);
    if expanded {
        push_line(
            lines,
            Line::from(Span::styled(
                "Environment (press e to collapse):",
                label.add_modifier(Modifier::BOLD),
            )),
        );
        if details.environment.is_empty() {
            push_line(lines, Line::from("  <unavailable>"));
        } else {
            for entry in &details.environment {
                push_line(lines, Line::from(format!("  {}", entry)));
            }
        }
    } else {
        push_line(
            lines,
            Line::from(Span::styled("Environment: (press e to expand)", label)),
        );
    }
}

fn build_open_files_section(
    lines: &mut Vec<Line>,
    palette: &Palette,
    expanded: bool,
    details: &ProcessDetails,
) {
    push_blank_line(lines);
    let label = label_style(palette);
    if expanded {
        push_line(
            lines,
            Line::from(Span::styled(
                "Open File Descriptors (press f to collapse):",
                label.add_modifier(Modifier::BOLD),
            )),
        );
        if details.open_files.is_empty() {
            push_line(lines, Line::from("  <unavailable>"));
        } else {
            for file in &details.open_files {
                push_line(lines, Line::from(format!("  {}", file)));
            }
        }
    } else {
        push_line(
            lines,
            Line::from(Span::styled("Open Files: (press f to expand)", label)),
        );
    }
}

fn build_network_section(
    lines: &mut Vec<Line>,
    palette: &Palette,
    expanded: bool,
    details: &ProcessDetails,
) {
    push_blank_line(lines);
    let label = label_style(palette);
    if expanded {
        push_line(
            lines,
            Line::from(Span::styled(
                "Network Connections (press n to collapse):",
                label.add_modifier(Modifier::BOLD),
            )),
        );
        if details.open_ports.is_empty() {
            push_line(lines, Line::from("  <unavailable>"));
        } else {
            for entry in &details.open_ports {
                push_line(lines, Line::from(format!("  {}", entry)));
            }
        }
    } else {
        push_line(
            lines,
            Line::from(Span::styled("Open Ports: (press n to expand)", label)),
        );
    }
}

fn build_cgroup_section(
    lines: &mut Vec<Line>,
    palette: &Palette,
    expanded: bool,
    details: &ProcessDetails,
) {
    push_blank_line(lines);
    let label = label_style(palette);
    if expanded {
        push_line(
            lines,
            Line::from(Span::styled(
                "Cgroups & Namespaces (press c to collapse):",
                label.add_modifier(Modifier::BOLD),
            )),
        );

        if details.cgroups.is_empty() {
            push_line(lines, Line::from("  <no cgroups>"));
        } else {
            for entry in &details.cgroups {
                push_line(lines, Line::from(format!("  {}", entry)));
            }
        }

        if details.namespaces.is_empty() {
            push_line(lines, Line::from("  <no namespaces>"));
        } else {
            for entry in &details.namespaces {
                push_line(lines, Line::from(format!("  {}", entry)));
            }
        }
    } else {
        push_line(
            lines,
            Line::from(Span::styled(
                "Cgroups & Namespaces: (press c to expand)",
                label,
            )),
        );
    }
}

fn push_line<'a>(lines: &mut Vec<Line<'a>>, line: Line<'a>) {
    lines.push(line);
}

fn push_blank_line(lines: &mut Vec<Line>) {
    if lines.last().map_or(false, |line| line.spans.is_empty()) {
        return;
    }
    lines.push(Line::default());
}

fn label_style(palette: &Palette) -> Style {
    Style::default()
        .fg(palette.text_dim)
        .add_modifier(Modifier::BOLD)
}

fn value_style(palette: &Palette) -> Style {
    Style::default().fg(palette.text_normal)
}
