use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, SortField, Theme};
use crate::process::{ProcessInfo, ProcessManager, matches_search};
use crate::signals::{Signal, SignalEvent, SignalSender};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AppMode {
    Normal,
    Search,
    SignalMenu,
    InfoPane,
    TreeView,
    HistoryView,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SortColumn {
    Cpu,
    Memory,
    Pid,
    Name,
    User,
    Runtime,
}

impl SortColumn {
    const ALL: [SortColumn; 6] = [
        SortColumn::Cpu,
        SortColumn::Memory,
        SortColumn::Pid,
        SortColumn::Name,
        SortColumn::User,
        SortColumn::Runtime,
    ];

    fn next(self) -> Self {
        let idx = SortColumn::ALL
            .iter()
            .position(|column| *column == self)
            .unwrap_or(0);
        let next_idx = (idx + 1) % SortColumn::ALL.len();
        SortColumn::ALL[next_idx]
    }

    fn prev(self) -> Self {
        let idx = SortColumn::ALL
            .iter()
            .position(|column| *column == self)
            .unwrap_or(0);
        let prev_idx = (idx + SortColumn::ALL.len() - 1) % SortColumn::ALL.len();
        SortColumn::ALL[prev_idx]
    }

    fn from_sort_field(field: SortField) -> Self {
        match field {
            SortField::Cpu => SortColumn::Cpu,
            SortField::Mem => SortColumn::Memory,
            SortField::Pid => SortColumn::Pid,
            SortField::Name => SortColumn::Name,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            SortColumn::Cpu => "CPU",
            SortColumn::Memory => "Memory",
            SortColumn::Pid => "PID",
            SortColumn::Name => "Name",
            SortColumn::User => "User",
            SortColumn::Runtime => "Runtime",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StatusLevel {
    Info,
    Warning,
    Error,
}

pub type SignalHistoryEntry = SignalEvent;

pub struct App {
    processes: Vec<ProcessInfo>,
    filtered_processes: Vec<ProcessInfo>,
    selected_index: usize,
    selected_pids: HashSet<u32>,

    mode: AppMode,
    search_query: String,
    sort_column: SortColumn,
    sort_descending: bool,
    show_all_processes: bool,

    info_pane_open: bool,
    tree_view_open: bool,
    signal_menu_open: bool,
    signal_menu_selected: usize,

    theme: Theme,
    refresh_rate_ms: u64,

    status_message: Option<(String, StatusLevel)>,
    signal_history: VecDeque<SignalHistoryEntry>,
    needs_refresh: bool,
    paused: bool,

    process_manager: ProcessManager,
    signal_sender: SignalSender,
}

impl App {
    pub fn new(config: Config) -> Self {
        let mut app = Self {
            processes: Vec::new(),
            filtered_processes: Vec::new(),
            selected_index: 0,
            selected_pids: HashSet::new(),
            mode: AppMode::Normal,
            search_query: config.initial_filter.clone().unwrap_or_default(),
            sort_column: SortColumn::from_sort_field(config.initial_sort),
            sort_descending: config.sort_descending,
            show_all_processes: config.show_all_processes,
            info_pane_open: false,
            tree_view_open: false,
            signal_menu_open: false,
            signal_menu_selected: 0,
            theme: config.theme,
            refresh_rate_ms: config.refresh_rate_ms,
            status_message: None,
            signal_history: VecDeque::with_capacity(10),
            needs_refresh: true,
            paused: false,
            process_manager: ProcessManager::new(),
            signal_sender: SignalSender::new(),
        };
        app.refresh_process_data();
        app.refresh_pause_state();
        app.update_signal_history();
        app
    }

    pub fn update_processes(&mut self) {
        if self.paused {
            return;
        }
        self.refresh_process_data();
    }

    pub fn apply_filters(&mut self) {
        let mut data = self.processes.clone();
        let query = self.search_query.trim();
        if !query.is_empty() {
            data.retain(|proc| matches_search(proc, query));
        }

        data.sort_by(|a, b| self.compare_processes(a, b));
        self.filtered_processes = data;
        self.selected_pids
            .retain(|pid| self.filtered_processes.iter().any(|proc| proc.pid == *pid));
        self.clamp_selection();
        self.needs_refresh = true;
    }

    pub fn handle_input(&mut self, event: KeyEvent) -> Result<bool> {
        let should_quit = match self.mode {
            AppMode::Search => self.handle_search_input(event)?,
            AppMode::SignalMenu => self.handle_signal_menu_input(event)?,
            _ => self.handle_normal_input(event)?,
        };
        Ok(should_quit)
    }

    pub fn select_next(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.filtered_processes.len();
        self.needs_refresh = true;
    }

    pub fn select_prev(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.filtered_processes.len() - 1;
        } else {
            self.selected_index -= 1;
        }
        self.needs_refresh = true;
    }

    pub fn toggle_selection(&mut self) {
        if let Some(pid) = self.current_pid() {
            if !self.selected_pids.remove(&pid) {
                self.selected_pids.insert(pid);
            }
            self.needs_refresh = true;
        }
    }

    pub fn kill_selected(&mut self, signal: Signal) {
        let targets = self.collect_target_pids();
        if targets.is_empty() {
            self.set_status(StatusLevel::Warning, "no process selected");
            return;
        }

        let mut successes = Vec::new();
        let mut errors = Vec::new();

        for pid in targets {
            match self.signal_sender.send_signal(pid, signal) {
                Ok(_) => {
                    successes.push(pid);
                    self.selected_pids.remove(&pid);
                }
                Err(err) => errors.push((pid, err)),
            }
        }

        self.update_signal_history();
        self.force_refresh_processes();

        if !successes.is_empty() {
            let message = format!("sent {} to {} process(es)", signal.name(), successes.len());
            self.set_status(StatusLevel::Info, message);
        }

        if let Some((pid, err)) = errors.first() {
            let message = format!("failed to send {} to {}: {}", signal.name(), pid, err);
            self.set_status(StatusLevel::Error, message);
        }
    }

    pub fn kill_selected_with_tree(&mut self, signal: Signal) {
        let targets = self.collect_target_pids();
        if targets.is_empty() {
            self.set_status(StatusLevel::Warning, "no process selected");
            return;
        }

        let mut successes = 0usize;
        let mut errors = Vec::new();

        for pid in targets {
            match self.signal_sender.kill_process_tree(pid, signal) {
                Ok(killed) => {
                    successes += killed.len();
                    self.selected_pids.remove(&pid);
                }
                Err(err) => errors.push((pid, err)),
            }
        }

        self.update_signal_history();
        self.force_refresh_processes();

        if successes > 0 {
            let message = format!(
                "sent {} with tree traversal to {} process(es)",
                signal.name(),
                successes
            );
            self.set_status(StatusLevel::Info, message);
        }

        if let Some((pid, err)) = errors.first() {
            let message = format!("tree kill failed for {}: {}", pid, err);
            self.set_status(StatusLevel::Error, message);
        }
    }

    pub fn jump_to_top(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        self.selected_index = 0;
        self.needs_refresh = true;
    }

    pub fn jump_to_bottom(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        self.selected_index = self.filtered_processes.len() - 1;
        self.needs_refresh = true;
    }

    pub fn needs_refresh(&self) -> bool {
        self.needs_refresh
    }

    pub fn signal_history(&self) -> &VecDeque<SignalHistoryEntry> {
        &self.signal_history
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    pub fn filtered_processes(&self) -> &[ProcessInfo] {
        &self.filtered_processes
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn status_message(&self) -> Option<&(String, StatusLevel)> {
        self.status_message.as_ref()
    }

    pub fn refresh_rate_ms(&self) -> u64 {
        self.refresh_rate_ms
    }

    fn handle_normal_input(&mut self, event: KeyEvent) -> Result<bool> {
        match event.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('/') => {
                self.set_mode(AppMode::Search);
                self.needs_refresh = true;
            }
            KeyCode::Char('i') => {
                self.info_pane_open = !self.info_pane_open;
                self.needs_refresh = true;
            }
            KeyCode::Char('t') => {
                self.tree_view_open = !self.tree_view_open;
                self.needs_refresh = true;
            }
            KeyCode::Char('s') => {
                self.signal_menu_open = true;
                self.signal_menu_selected = 0;
                self.set_mode(AppMode::SignalMenu);
            }
            KeyCode::Char('x') => self.kill_selected_with_tree(Signal::Sigterm),
            KeyCode::Char('k') => self.kill_selected(Signal::Sigterm),
            KeyCode::Char('K') => self.kill_selected(Signal::Sigkill),
            KeyCode::Char('g') => self.jump_to_top(),
            KeyCode::Char('G') => self.jump_to_bottom(),
            KeyCode::Char('<') => {
                self.sort_column = self.sort_column.prev();
                self.apply_filters();
                let message = format!(
                    "sorting by {} {}",
                    self.sort_column.display_name(),
                    order_text(self.sort_descending)
                );
                self.set_status(StatusLevel::Info, message);
            }
            KeyCode::Char('>') => {
                self.sort_column = self.sort_column.next();
                self.apply_filters();
                let message = format!(
                    "sorting by {} {}",
                    self.sort_column.display_name(),
                    order_text(self.sort_descending)
                );
                self.set_status(StatusLevel::Info, message);
            }
            KeyCode::Char('?') => {
                self.set_status(StatusLevel::Info, "help not yet implemented");
            }
            KeyCode::Char(' ') => self.toggle_selection(),
            KeyCode::Enter => self.kill_selected(Signal::Sigterm),
            KeyCode::Up => self.select_prev(),
            KeyCode::Down => self.select_next(),
            _ => {}
        }
        Ok(false)
    }

    fn handle_search_input(&mut self, event: KeyEvent) -> Result<bool> {
        match event.code {
            KeyCode::Esc => {
                self.set_mode(AppMode::Normal);
            }
            KeyCode::Enter => {
                self.set_mode(AppMode::Normal);
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.apply_filters();
            }
            KeyCode::Char(c)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.search_query.push(c);
                self.apply_filters();
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_signal_menu_input(&mut self, event: KeyEvent) -> Result<bool> {
        let total = Signal::all().len();
        if total == 0 {
            self.set_mode(AppMode::Normal);
            return Ok(false);
        }

        match event.code {
            KeyCode::Esc => {
                self.signal_menu_open = false;
                self.set_mode(AppMode::Normal);
            }
            KeyCode::Up => {
                if self.signal_menu_selected == 0 {
                    self.signal_menu_selected = total - 1;
                } else {
                    self.signal_menu_selected -= 1;
                }
                self.needs_refresh = true;
            }
            KeyCode::Down => {
                self.signal_menu_selected = (self.signal_menu_selected + 1) % total;
                self.needs_refresh = true;
            }
            KeyCode::Enter => {
                let signal = Signal::all()[self.signal_menu_selected];
                self.signal_menu_open = false;
                self.set_mode(AppMode::Normal);
                self.kill_selected(signal);
            }
            _ => {}
        }
        Ok(false)
    }

    fn refresh_process_data(&mut self) {
        self.processes = self.process_manager.get_processes(self.show_all_processes);
        self.selected_pids
            .retain(|pid| self.processes.iter().any(|proc| proc.pid == *pid));
        self.apply_filters();
    }

    fn force_refresh_processes(&mut self) {
        let paused = self.paused;
        self.paused = false;
        self.refresh_process_data();
        self.paused = paused;
    }

    fn compare_processes(&self, a: &ProcessInfo, b: &ProcessInfo) -> Ordering {
        let ordering = match self.sort_column {
            SortColumn::Cpu => a
                .cpu_percent
                .partial_cmp(&b.cpu_percent)
                .unwrap_or(Ordering::Equal),
            SortColumn::Memory => a.memory_bytes.cmp(&b.memory_bytes),
            SortColumn::Pid => a.pid.cmp(&b.pid),
            SortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortColumn::User => a.user.to_lowercase().cmp(&b.user.to_lowercase()),
            SortColumn::Runtime => a.runtime.cmp(&b.runtime),
        };

        if self.sort_descending {
            ordering.reverse()
        } else {
            ordering
        }
    }

    fn clamp_selection(&mut self) {
        if self.filtered_processes.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.filtered_processes.len() {
            self.selected_index = self.filtered_processes.len() - 1;
        }
    }

    fn current_pid(&self) -> Option<u32> {
        self.filtered_processes
            .get(self.selected_index)
            .map(|proc| proc.pid)
    }

    fn collect_target_pids(&self) -> Vec<u32> {
        if self.selected_pids.is_empty() {
            return self.current_pid().into_iter().collect();
        }

        let mut targets: Vec<u32> = self
            .filtered_processes
            .iter()
            .filter(|proc| self.selected_pids.contains(&proc.pid))
            .map(|proc| proc.pid)
            .collect();

        for pid in &self.selected_pids {
            if !targets.contains(pid) && self.processes.iter().any(|proc| proc.pid == *pid) {
                targets.push(*pid);
            }
        }

        targets
    }

    fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
        self.refresh_pause_state();
        self.needs_refresh = true;
    }

    fn refresh_pause_state(&mut self) {
        self.paused = matches!(self.mode, AppMode::Search | AppMode::SignalMenu);
    }

    fn set_status<T: Into<String>>(&mut self, level: StatusLevel, message: T) {
        self.status_message = Some((message.into(), level));
        self.needs_refresh = true;
    }

    fn update_signal_history(&mut self) {
        let entries: Vec<_> = self.signal_sender.history().cloned().collect();
        let mut deque = VecDeque::with_capacity(10);
        for entry in entries.into_iter().take(10) {
            deque.push_back(entry);
        }
        self.signal_history = deque;
    }
}

fn order_text(desc: bool) -> &'static str {
    if desc { "(desc)" } else { "(asc)" }
}
