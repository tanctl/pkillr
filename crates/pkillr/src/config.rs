use clap::ValueEnum;
use ratatui::style::{Color, Style};

use crate::process::ProcessInfo;

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum Theme {
    Pink,
    Serious,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Pink
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub table_border: Color,
    pub table_header: Color,
    pub text_normal: Color,
    pub text_dim: Color,
    pub highlight_selected: Color,
    pub cpu_yellow: Color,
    pub cpu_red: Color,
    pub mem_yellow: Color,
    pub mem_red: Color,
    pub kill_accent: Color,
    pub status_info: Color,
    pub status_warning: Color,
    pub status_error: Color,
}

const CPU_YELLOW_THRESHOLD: f32 = 40.0;
const CPU_RED_THRESHOLD: f32 = 80.0;
const MEM_YELLOW_THRESHOLD: u64 = 500 * 1024 * 1024;
const MEM_RED_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024;

impl Theme {
    pub fn palette(self) -> Palette {
        match self {
            Theme::Pink => Palette {
                background: Color::Black,
                table_border: Color::Rgb(255, 20, 147),
                table_header: Color::Rgb(255, 20, 147),
                text_normal: Color::Rgb(255, 20, 147),
                text_dim: Color::Rgb(199, 21, 133),
                highlight_selected: Color::Rgb(199, 21, 133),
                cpu_yellow: Color::Rgb(255, 105, 180),
                cpu_red: Color::Rgb(255, 0, 120),
                mem_yellow: Color::Rgb(255, 105, 180),
                mem_red: Color::Rgb(255, 0, 120),
                kill_accent: Color::Rgb(255, 20, 147),
                status_info: Color::Rgb(255, 20, 147),
                status_warning: Color::Rgb(255, 105, 180),
                status_error: Color::Rgb(255, 0, 120),
            },
            Theme::Serious => Palette {
                background: Color::Black,
                table_border: Color::White,
                table_header: Color::Rgb(100, 100, 100),
                text_normal: Color::White,
                text_dim: Color::Rgb(100, 100, 100),
                highlight_selected: Color::Rgb(0, 255, 255),
                cpu_yellow: Color::Yellow,
                cpu_red: Color::Red,
                mem_yellow: Color::Yellow,
                mem_red: Color::Red,
                kill_accent: Color::Red,
                status_info: Color::Blue,
                status_warning: Color::Yellow,
                status_error: Color::Red,
            },
        }
    }

    pub fn get_cpu_color(self, percent: f32) -> Color {
        let palette = self.palette();
        if percent >= CPU_RED_THRESHOLD {
            palette.cpu_red
        } else if percent >= CPU_YELLOW_THRESHOLD {
            palette.cpu_yellow
        } else {
            palette.text_normal
        }
    }

    pub fn get_memory_color(self, bytes: u64) -> Color {
        let palette = self.palette();
        if bytes >= MEM_RED_THRESHOLD {
            palette.mem_red
        } else if bytes >= MEM_YELLOW_THRESHOLD {
            palette.mem_yellow
        } else {
            palette.text_normal
        }
    }

    pub fn style_for_process(self, proc: &ProcessInfo) -> Style {
        let palette = self.palette();
        let cpu_color = self.get_cpu_color(proc.cpu_percent);
        let mem_color = self.get_memory_color(proc.memory_bytes);
        let fg = if cpu_color == palette.cpu_red || mem_color == palette.mem_red {
            palette.cpu_red
        } else if cpu_color == palette.cpu_yellow || mem_color == palette.mem_yellow {
            palette.cpu_yellow
        } else {
            palette.text_normal
        };
        Style::default().fg(fg).bg(palette.background)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum SortField {
    Cpu,
    Mem,
    Pid,
    Name,
}

impl Default for SortField {
    fn default() -> Self {
        SortField::Cpu
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub theme: Theme,
    pub show_all_processes: bool,
    pub refresh_rate_ms: u64,
    pub initial_filter: Option<String>,
    pub initial_sort: SortField,
    pub sort_descending: bool,
}
