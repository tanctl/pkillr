use clap::ValueEnum;

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
