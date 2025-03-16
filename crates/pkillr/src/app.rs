use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nix::unistd::{Uid, getppid};

use crate::config::{Config, SortField, Theme};
use crate::process::{ProcessDetails, ProcessInfo, ProcessManager, can_kill, get_process_tree};
use crate::signals::{Signal, SignalEvent, SignalSender};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use regex::{Regex, RegexBuilder};

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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum RiskLevel {
    Elevated,
    Critical,
}

#[derive(Debug, Clone)]
pub struct RiskInfo {
    pub level: RiskLevel,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub subtree_cpu: f32,
    pub subtree_memory_bytes: u64,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
    pub prefix: String,
    pub risk: Option<RiskInfo>,
}

#[derive(Debug, Clone)]
pub struct TreeKillPrompt {
    pub pid: u32,
    pub signal: Signal,
    pub lines: Vec<String>,
    pub risk: Option<RiskInfo>,
}

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
enum SearchMode {
    Fuzzy(String),
    Regex {
        pattern: String,
        flags: String,
        matcher: Regex,
    },
    History(String),
}

#[derive(Debug, Clone)]
struct SearchHit {
    score: i64,
    name_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
enum PendingKill {
    Direct { targets: Vec<u32>, signal: Signal },
    Tree { targets: Vec<u32>, signal: Signal },
}

#[derive(Debug, Clone, Copy)]
enum KillMode {
    Direct,
    Tree,
}

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
    signal_menu_scroll_offset: usize,
    signal_menu_target: Option<u32>,
    shell_confirm: Option<PendingKill>,
    history_popup_open: bool,
    help_popup_open: bool,
    search_pending: bool,
    last_search_edit: Option<Instant>,
    search_matches: HashMap<u32, Vec<usize>>,
    search_scores: HashMap<u32, i64>,
    mode_before_popup: Option<AppMode>,

    theme: Theme,
    refresh_rate_ms: u64,

    status_message: Option<(String, StatusLevel)>,
    signal_history: VecDeque<SignalHistoryEntry>,
    needs_refresh: bool,
    paused: bool,

    info_pane_scroll: u16,
    info_focus: bool,
    info_env_expanded: bool,
    info_files_expanded: bool,
    info_maps_expanded: bool,
    info_network_expanded: bool,
    info_cgroups_expanded: bool,
    info_details_cache: Option<(u32, ProcessDetails)>,

    table_scroll_offset: usize,
    tree_selected_index: usize,
    tree_rows: Vec<TreeRow>,
    tree_collapsed: HashSet<u32>,
    tree_scroll_offset: usize,
    tree_kill_prompt: Option<TreeKillPrompt>,
    is_root: bool,
    parent_pid: u32,
    total_memory_bytes: u64,

    process_manager: ProcessManager,
    signal_sender: SignalSender,
}

impl App {
    pub fn new(config: Config) -> Self {
        let current_uid = Uid::current();
        let is_root = current_uid.as_raw() == 0;

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
            signal_menu_scroll_offset: 0,
            signal_menu_target: None,
            shell_confirm: None,
            history_popup_open: false,
            help_popup_open: false,
            search_pending: false,
            last_search_edit: None,
            search_matches: HashMap::new(),
            search_scores: HashMap::new(),
            mode_before_popup: None,
            theme: config.theme,
            refresh_rate_ms: config.refresh_rate_ms,
            status_message: None,
            signal_history: VecDeque::with_capacity(10),
            needs_refresh: true,
            paused: false,
            info_pane_scroll: 0,
            info_focus: false,
            info_env_expanded: false,
            info_files_expanded: false,
            info_maps_expanded: false,
            info_network_expanded: false,
            info_cgroups_expanded: false,
            info_details_cache: None,
            table_scroll_offset: 0,
            tree_selected_index: 0,
            tree_rows: Vec::new(),
            tree_collapsed: HashSet::new(),
            tree_scroll_offset: 0,
            tree_kill_prompt: None,
            is_root,
            parent_pid: getppid().as_raw() as u32,
            total_memory_bytes: 0,
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
        let raw_query = self.search_query.trim().to_string();
        self.search_matches.clear();
        self.search_scores.clear();

        let mode = match Self::parse_search_mode(&raw_query) {
            Ok(mode) => mode,
            Err(err) => {
                self.filtered_processes.clear();
                self.selected_pids.clear();
                self.table_scroll_offset = 0;
                self.set_status(StatusLevel::Error, err);
                self.invalidate_process_details();
                self.search_pending = false;
                self.last_search_edit = None;
                self.needs_refresh = true;
                return;
            }
        };

        match &mode {
            SearchMode::Fuzzy(query) => {
                if !query.is_empty() {
                    let matcher = SkimMatcherV2::default();
                    data = data
                        .into_iter()
                        .filter_map(|proc| {
                            fuzzy_match_process(&proc, query, &matcher).map(|hit| {
                                if !hit.name_indices.is_empty() {
                                    self.search_matches.insert(proc.pid, hit.name_indices);
                                }
                                self.search_scores.insert(proc.pid, hit.score);
                                proc
                            })
                        })
                        .collect();
                }
            }
            SearchMode::Regex { matcher, .. } => {
                let regex = matcher.clone();
                data = data
                    .into_iter()
                    .filter_map(|proc| {
                        regex_match_process(&proc, &regex).map(|hit| {
                            if !hit.name_indices.is_empty() {
                                self.search_matches.insert(proc.pid, hit.name_indices);
                            }
                            self.search_scores.insert(proc.pid, hit.score);
                            proc
                        })
                    })
                    .collect();
            }
            SearchMode::History(filter) => {
                data = self.filter_by_history(data, filter);
            }
        }

        let mut sort_by_score = !self.search_scores.is_empty();
        if matches!(mode, SearchMode::Fuzzy(ref query) if query.is_empty()) {
            sort_by_score = false;
        }

        if sort_by_score {
            data.sort_by(|a, b| {
                let score_a = self.search_scores.get(&a.pid).copied().unwrap_or(0);
                let score_b = self.search_scores.get(&b.pid).copied().unwrap_or(0);
                score_b
                    .cmp(&score_a)
                    .then_with(|| self.compare_processes(a, b))
            });
        } else {
            data.sort_by(|a, b| self.compare_processes(a, b));
        }

        let previous_len = self.filtered_processes.len();
        self.filtered_processes = data;
        self.selected_pids
            .retain(|pid| self.filtered_processes.iter().any(|proc| proc.pid == *pid));
        self.clamp_selection();
        if self.filtered_processes.is_empty() {
            self.table_scroll_offset = 0;
            let message = match mode {
                SearchMode::Fuzzy(query) if query.is_empty() => "No processes found".to_string(),
                SearchMode::Fuzzy(query) => format!("No matches for '{}'", query),
                SearchMode::Regex { pattern, flags, .. } => {
                    let rendered = if flags.is_empty() {
                        format!("/{pattern}/")
                    } else {
                        format!("/{pattern}/{}", flags)
                    };
                    format!("No regex matches for {}", rendered)
                }
                SearchMode::History(filter) if filter.is_empty() => {
                    "No recent signal history".to_string()
                }
                SearchMode::History(filter) => {
                    format!("No history entries matching '{}'", filter)
                }
            };
            self.set_status(StatusLevel::Info, message);
        } else {
            let max_offset = self.filtered_processes.len().saturating_sub(1);
            self.table_scroll_offset = self.table_scroll_offset.min(max_offset);
            if previous_len == 0 {
                match mode {
                    SearchMode::Fuzzy(query) if !query.is_empty() => {
                        let message = format!("Showing matches for '{}'", query);
                        self.set_status(StatusLevel::Info, message);
                    }
                    SearchMode::Regex { pattern, flags, .. } => {
                        let rendered = if flags.is_empty() {
                            format!("/{pattern}/")
                        } else {
                            format!("/{pattern}/{}", flags)
                        };
                        let message = format!("Regex filter active: {}", rendered);
                        self.set_status(StatusLevel::Info, message);
                    }
                    SearchMode::History(filter) => {
                        let message = if filter.is_empty() {
                            "Showing processes with recent signals".to_string()
                        } else {
                            format!("History filter active: '{}'", filter)
                        };
                        self.set_status(StatusLevel::Info, message);
                    }
                    _ => {}
                }
            }
        }
        self.invalidate_process_details();
        self.search_pending = false;
        self.last_search_edit = None;
        self.needs_refresh = true;
    }

    pub fn tick(&mut self, now: Instant) {
        if self.search_pending {
            if let Some(last) = self.last_search_edit {
                if now.saturating_duration_since(last) >= SEARCH_DEBOUNCE {
                    self.apply_filters();
                }
            } else {
                self.apply_filters();
            }
        }
    }

    fn mark_search_dirty(&mut self) {
        self.search_pending = true;
        self.last_search_edit = Some(Instant::now());
        self.apply_filters();
    }

    fn flush_search_filters(&mut self) {
        if self.search_pending {
            self.apply_filters();
        }
    }

    pub fn handle_input(&mut self, event: KeyEvent) -> Result<bool> {
        if let Some(result) = self.handle_shell_confirm_input(event)? {
            return Ok(result);
        }
        if self.help_popup_open {
            return self.handle_help_popup_input(event);
        }
        if self.history_popup_open {
            return self.handle_history_popup_input(event);
        }

        let should_quit = match self.mode {
            AppMode::Search => self.handle_search_input(event)?,
            AppMode::SignalMenu => self.handle_signal_menu_input(event)?,
            AppMode::TreeView => self.handle_tree_input(event)?,
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
        self.invalidate_process_details();
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
        self.invalidate_process_details();
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
        if !self.dispatch_signal_targets(targets, signal, KillMode::Direct, false) {
            return;
        }
    }

    pub fn kill_selected_with_tree(&mut self, signal: Signal) {
        let targets = self.collect_target_pids();
        if !self.dispatch_signal_targets(targets, signal, KillMode::Tree, false) {
            return;
        }
    }

    fn dispatch_signal_targets(
        &mut self,
        targets: Vec<u32>,
        signal: Signal,
        mode: KillMode,
        allow_shell_override: bool,
    ) -> bool {
        if targets.is_empty() {
            self.set_status(StatusLevel::Warning, "no process selected");
            return false;
        }

        if !allow_shell_override && !self.is_root {
            if targets.iter().any(|pid| *pid == self.parent_pid) {
                self.shell_confirm = Some(match mode {
                    KillMode::Direct => PendingKill::Direct { targets, signal },
                    KillMode::Tree => PendingKill::Tree { targets, signal },
                });
                self.set_status(
                    StatusLevel::Warning,
                    format!(
                        "This is your shell process (PID {}). Continue? (y/n)",
                        self.parent_pid
                    ),
                );
                self.needs_refresh = true;
                self.refresh_pause_state();
                return false;
            }
        }

        let executed = match mode {
            KillMode::Direct => self.dispatch_direct(targets, signal),
            KillMode::Tree => self.dispatch_tree(targets, signal),
        };

        self.needs_refresh = true;
        self.refresh_pause_state();
        executed
    }

    fn dispatch_direct(&mut self, targets: Vec<u32>, signal: Signal) -> bool {
        let mut successes = Vec::new();
        let mut errors = Vec::new();

        for pid in targets {
            let name = self
                .process_name_for_pid(pid)
                .unwrap_or_else(|| format!("PID {pid}"));
            let risk = self.risk_for_pid(pid);
            match self.signal_sender.send_signal(pid, signal) {
                Ok(_) => {
                    successes.push((pid, name, risk));
                    self.selected_pids.remove(&pid);
                }
                Err(err) => errors.push((pid, name, err)),
            }
        }

        self.update_signal_history();
        self.force_refresh_processes();
        self.invalidate_process_details();

        if errors.is_empty() {
            if !successes.is_empty() {
                self.report_kill_success(&successes, signal);
            }
        } else {
            let (_, _, err) = &errors[0];
            self.report_kill_error(err);
        }

        !successes.is_empty() || !errors.is_empty()
    }

    fn dispatch_tree(&mut self, targets: Vec<u32>, signal: Signal) -> bool {
        let mut total_killed = 0usize;
        let mut errors = Vec::new();
        let mut risk_notes = Vec::new();

        for pid in targets {
            if let Some(risk) = self.risk_for_pid(pid) {
                risk_notes.push(risk);
            }
            match self.signal_sender.kill_process_tree(pid, signal) {
                Ok(killed) => {
                    total_killed += killed.len();
                    self.selected_pids.remove(&pid);
                }
                Err(err) => {
                    errors.push(err);
                    break;
                }
            }
        }

        self.update_signal_history();
        self.force_refresh_processes();
        self.invalidate_process_details();

        if errors.is_empty() {
            if total_killed > 0 {
                if let Some(risk) = risk_notes.iter().max_by_key(|info| info.level) {
                    let mut level = match risk.level {
                        RiskLevel::Critical => StatusLevel::Error,
                        RiskLevel::Elevated => StatusLevel::Warning,
                    };
                    if level == StatusLevel::Info && is_dangerous_signal(signal) {
                        level = StatusLevel::Warning;
                    }
                    let message = format!(
                        "Killed process tree: {} processes terminated — caution: {}",
                        total_killed, risk.reason
                    );
                    self.set_status(level, message);
                } else {
                    let mut level = StatusLevel::Info;
                    if is_dangerous_signal(signal) {
                        level = StatusLevel::Warning;
                    }
                    self.set_status(
                        level,
                        format!("Killed process tree: {} processes terminated", total_killed),
                    );
                }
            }
        } else if let Some(err) = errors.first() {
            self.report_kill_error(err);
        }

        total_killed > 0 || !errors.is_empty()
    }

    fn report_kill_success(
        &mut self,
        successes: &[(u32, String, Option<RiskInfo>)],
        signal: Signal,
    ) {
        if successes.is_empty() {
            return;
        }
        let highest_risk = successes
            .iter()
            .filter_map(|(_, _, risk)| risk.as_ref())
            .max_by_key(|info| info.level)
            .cloned();
        let base_level = highest_risk
            .as_ref()
            .map(|risk| match risk.level {
                RiskLevel::Critical => StatusLevel::Error,
                RiskLevel::Elevated => StatusLevel::Warning,
            })
            .unwrap_or(StatusLevel::Info);

        let message = if successes.len() == 1 {
            let (pid, name, _) = &successes[0];
            if let Some(risk) = highest_risk {
                format!(
                    "Killed {} (PID {}) with {} — caution: {}",
                    name,
                    pid,
                    signal.name(),
                    risk.reason
                )
            } else {
                format!("Killed {} (PID {}) with {}", name, pid, signal.name())
            }
        } else if let Some(risk) = highest_risk {
            format!(
                "Killed {} processes with {} — caution: {}",
                successes.len(),
                signal.name(),
                risk.reason
            )
        } else {
            format!(
                "Killed {} processes with {}",
                successes.len(),
                signal.name()
            )
        };

        let mut level = base_level;
        if level == StatusLevel::Info && is_dangerous_signal(signal) {
            level = StatusLevel::Warning;
        }
        self.set_status(level, message);
    }

    fn report_kill_error(&mut self, error: &str) {
        let message = self.friendly_error_message(error);
        self.set_status(StatusLevel::Error, message);
    }

    pub(crate) fn friendly_error_message(&self, error: &str) -> String {
        let lowered = error.to_ascii_lowercase();
        if lowered.contains("permission") {
            "Permission denied. Run with sudo or select a user-owned process.".to_string()
        } else if lowered.contains("pid 1") {
            "Cannot kill init process".to_string()
        } else if lowered.contains("pkillr") {
            "Cannot kill pkillr itself".to_string()
        } else if lowered.contains("shell") && lowered.contains("parent") {
            "Refusing to kill your current shell".to_string()
        } else {
            error.to_string()
        }
    }

    pub fn jump_to_top(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        self.selected_index = 0;
        self.needs_refresh = true;
        self.invalidate_process_details();
    }

    pub fn jump_to_bottom(&mut self) {
        if self.filtered_processes.is_empty() {
            return;
        }
        self.selected_index = self.filtered_processes.len() - 1;
        self.needs_refresh = true;
        self.invalidate_process_details();
    }

    pub fn needs_refresh(&self) -> bool {
        self.needs_refresh
    }

    pub fn clear_refresh_flag(&mut self) {
        self.needs_refresh = false;
    }

    pub fn request_redraw(&mut self) {
        self.needs_refresh = true;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn mode(&self) -> AppMode {
        self.mode
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
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

    pub fn highlight_indices(&self, pid: u32) -> Option<&[usize]> {
        self.search_matches
            .get(&pid)
            .map(|indices| indices.as_slice())
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_pids.is_empty()
    }

    pub fn is_pid_selected(&self, pid: u32) -> bool {
        self.selected_pids.contains(&pid)
    }

    pub fn can_kill_without_privileges(&self, proc: &ProcessInfo) -> bool {
        can_kill(proc).is_ok()
    }

    pub fn table_scroll_offset(&self) -> usize {
        self.table_scroll_offset
    }

    pub fn set_table_scroll_offset(&mut self, offset: usize) {
        self.table_scroll_offset = offset;
    }

    pub fn status_message(&self) -> Option<&(String, StatusLevel)> {
        self.status_message.as_ref()
    }

    pub fn refresh_rate_ms(&self) -> u64 {
        self.refresh_rate_ms
    }

    pub fn total_memory_bytes(&self) -> u64 {
        self.total_memory_bytes
    }

    pub fn signal_menu_open(&self) -> bool {
        self.signal_menu_open
    }

    pub fn signal_menu_selected(&self) -> usize {
        self.signal_menu_selected
    }

    pub fn signal_menu_scroll_offset(&self) -> usize {
        self.signal_menu_scroll_offset
    }

    pub fn set_signal_menu_scroll_offset(&mut self, offset: usize) {
        self.signal_menu_scroll_offset = offset;
    }

    pub fn signal_menu_target(&self) -> Option<u32> {
        self.signal_menu_target
    }

    pub fn history_popup_open(&self) -> bool {
        self.history_popup_open
    }

    pub fn help_popup_open(&self) -> bool {
        self.help_popup_open
    }

    pub fn tree_view_open(&self) -> bool {
        self.tree_view_open
    }

    pub fn tree_rows(&self) -> &[TreeRow] {
        &self.tree_rows
    }

    pub fn tree_scroll_offset(&self) -> usize {
        self.tree_scroll_offset
    }

    pub fn set_tree_scroll_offset(&mut self, offset: usize) {
        self.tree_scroll_offset = offset;
    }

    pub fn tree_selected_index(&self) -> usize {
        self.tree_selected_index
    }

    pub fn tree_selected_pid(&self) -> Option<u32> {
        self.tree_rows
            .get(self.tree_selected_index)
            .map(|row| row.pid)
    }

    pub fn tree_kill_prompt(&self) -> Option<&TreeKillPrompt> {
        self.tree_kill_prompt.as_ref()
    }

    pub fn is_info_pane_open(&self) -> bool {
        self.info_pane_open
    }

    pub fn info_focus(&self) -> bool {
        self.info_focus
    }

    pub fn toggle_info_focus(&mut self) {
        if self.info_pane_open {
            self.info_focus = !self.info_focus;
            self.needs_refresh = true;
        }
    }

    pub fn toggle_info_pane(&mut self) {
        self.info_pane_open = !self.info_pane_open;
        self.info_focus = false;
        self.info_pane_scroll = 0;
        self.invalidate_process_details();
        let mut adjusted_mode = false;
        if self.info_pane_open {
            if !self.tree_view_open
                && !matches!(
                    self.mode,
                    AppMode::Search | AppMode::SignalMenu | AppMode::InfoPane
                )
            {
                self.set_mode(AppMode::InfoPane);
                adjusted_mode = true;
            }
        } else if matches!(self.mode, AppMode::InfoPane) {
            let next_mode = if self.tree_view_open {
                AppMode::TreeView
            } else {
                AppMode::Normal
            };
            self.set_mode(next_mode);
            adjusted_mode = true;
        }

        if !adjusted_mode {
            self.needs_refresh = true;
        }
    }

    pub fn info_pane_scroll(&self) -> u16 {
        self.info_pane_scroll
    }

    pub fn scroll_info_pane(&mut self, delta: i16) {
        if !self.info_pane_open {
            return;
        }
        let current = self.info_pane_scroll as i32;
        let new = current + delta as i32;
        self.info_pane_scroll = if new <= 0 {
            0
        } else if new >= u16::MAX as i32 {
            u16::MAX
        } else {
            new as u16
        };
        self.needs_refresh = true;
    }

    pub fn info_env_expanded(&self) -> bool {
        self.info_env_expanded
    }

    pub fn toggle_info_env(&mut self) {
        if !self.info_pane_open {
            return;
        }
        self.info_env_expanded = !self.info_env_expanded;
        self.info_pane_scroll = 0;
        self.needs_refresh = true;
    }

    pub fn info_files_expanded(&self) -> bool {
        self.info_files_expanded
    }

    pub fn toggle_info_files(&mut self) {
        if !self.info_pane_open {
            return;
        }
        self.info_files_expanded = !self.info_files_expanded;
        self.info_pane_scroll = 0;
        self.needs_refresh = true;
    }

    pub fn info_maps_expanded(&self) -> bool {
        self.info_maps_expanded
    }

    pub fn toggle_info_maps(&mut self) {
        if !self.info_pane_open {
            return;
        }
        self.info_maps_expanded = !self.info_maps_expanded;
        self.info_pane_scroll = 0;
        self.needs_refresh = true;
    }

    pub fn info_network_expanded(&self) -> bool {
        self.info_network_expanded
    }

    pub fn toggle_info_network(&mut self) {
        if !self.info_pane_open {
            return;
        }
        self.info_network_expanded = !self.info_network_expanded;
        self.info_pane_scroll = 0;
        self.needs_refresh = true;
    }

    pub fn info_cgroups_expanded(&self) -> bool {
        self.info_cgroups_expanded
    }

    pub fn toggle_info_cgroups(&mut self) {
        if !self.info_pane_open {
            return;
        }
        self.info_cgroups_expanded = !self.info_cgroups_expanded;
        self.info_pane_scroll = 0;
        self.needs_refresh = true;
    }

    pub fn process_details(&mut self) -> Option<&ProcessDetails> {
        let pid = self.current_pid()?;
        if !self.info_pane_open {
            return None;
        }

        let cached_pid = self.info_details_cache.as_ref().map(|(cached, _)| *cached);
        if cached_pid != Some(pid) {
            match self.process_manager.get_details(pid) {
                Some(details) => {
                    self.info_details_cache = Some((pid, details));
                }
                None => {
                    self.info_details_cache = None;
                    return None;
                }
            }
        }

        self.info_details_cache.as_ref().map(|(_, details)| details)
    }

    fn process_name_for_pid(&self, pid: u32) -> Option<String> {
        self.processes
            .iter()
            .find(|proc| proc.pid == pid)
            .map(|proc| proc.name.clone())
            .or_else(|| {
                self.tree_rows
                    .iter()
                    .find(|row| row.pid == pid)
                    .map(|row| row.name.clone())
            })
    }

    fn open_signal_menu(&mut self, target: Option<u32>) {
        self.signal_menu_open = true;
        self.signal_menu_target = target;
        if let Some(default_idx) = Signal::all()
            .iter()
            .position(|sig| matches!(sig, Signal::Sigterm))
        {
            self.signal_menu_selected = default_idx;
        } else if self.signal_menu_selected >= Signal::all().len() {
            self.signal_menu_selected = 0;
        }
        self.signal_menu_scroll_offset = 0;
        self.set_mode(AppMode::SignalMenu);
        self.needs_refresh = true;
    }

    fn close_signal_menu(&mut self) {
        self.signal_menu_open = false;
        self.signal_menu_scroll_offset = 0;
        self.signal_menu_target = None;
        if self.tree_view_open {
            self.set_mode(AppMode::TreeView);
        } else {
            self.set_mode(AppMode::Normal);
        }
        self.needs_refresh = true;
    }

    fn open_history_popup(&mut self) {
        if self.history_popup_open {
            return;
        }
        if self.mode_before_popup.is_none() {
            self.mode_before_popup = Some(self.mode);
        }
        self.history_popup_open = true;
        self.set_mode(AppMode::HistoryView);
    }

    fn close_history_popup(&mut self) {
        if !self.history_popup_open {
            return;
        }
        self.history_popup_open = false;
        self.restore_mode_after_overlay();
    }

    fn open_help_popup(&mut self) {
        if self.help_popup_open {
            return;
        }
        if self.mode_before_popup.is_none() {
            self.mode_before_popup = Some(self.mode);
        }
        self.help_popup_open = true;
        self.refresh_pause_state();
        self.needs_refresh = true;
    }

    fn close_help_popup(&mut self) {
        if !self.help_popup_open {
            return;
        }
        self.help_popup_open = false;
        self.restore_mode_after_overlay();
    }

    fn restore_mode_after_overlay(&mut self) {
        if self.history_popup_open {
            self.set_mode(AppMode::HistoryView);
            return;
        }

        if self.shell_confirm.is_some() {
            self.refresh_pause_state();
            self.needs_refresh = true;
            return;
        }

        if self.tree_view_open {
            self.set_mode(AppMode::TreeView);
            self.mode_before_popup = None;
            return;
        }

        if self.info_pane_open && !matches!(self.mode, AppMode::Search | AppMode::SignalMenu) {
            self.set_mode(AppMode::InfoPane);
            self.mode_before_popup = None;
        } else if let Some(previous) = self.mode_before_popup.take() {
            self.set_mode(previous);
        } else if !matches!(self.mode, AppMode::Search | AppMode::SignalMenu) {
            self.set_mode(AppMode::Normal);
        } else {
            self.refresh_pause_state();
            self.needs_refresh = true;
        }
    }

    fn send_signal_from_menu(&mut self, signal: Signal) {
        let target = self.signal_menu_target.or_else(|| {
            if self.tree_view_open {
                self.tree_selected_pid()
            } else {
                self.current_pid()
            }
        });

        let Some(pid) = target else {
            self.set_status(StatusLevel::Warning, "no process selected");
            self.close_signal_menu();
            return;
        };
        let executed = self.dispatch_signal_targets(vec![pid], signal, KillMode::Direct, false);
        self.close_signal_menu();
        if executed {
            self.invalidate_process_details();
        }
    }

    fn handle_history_popup_input(&mut self, _event: KeyEvent) -> Result<bool> {
        self.close_history_popup();
        Ok(false)
    }

    fn handle_help_popup_input(&mut self, _event: KeyEvent) -> Result<bool> {
        self.close_help_popup();
        Ok(false)
    }

    pub fn toggle_tree_view(&mut self) {
        self.tree_view_open = !self.tree_view_open;
        if self.tree_view_open {
            self.info_pane_open = false;
            self.info_focus = false;
            self.tree_collapsed.clear();
            self.tree_rows.clear();
            self.tree_selected_index = 0;
            self.tree_scroll_offset = 0;
            self.tree_kill_prompt = None;
            self.rebuild_tree_nodes();
            self.set_mode(AppMode::TreeView);
        } else {
            self.tree_kill_prompt = None;
            self.tree_rows.clear();
            self.tree_collapsed.clear();
            self.tree_scroll_offset = 0;
            self.set_mode(AppMode::Normal);
        }
        self.needs_refresh = true;
    }

    fn handle_tree_input(&mut self, event: KeyEvent) -> Result<bool> {
        if let Some(_) = self.tree_kill_prompt {
            match event.code {
                KeyCode::Char('y') => {
                    self.tree_kill_preview_confirm(true);
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.tree_kill_preview_confirm(false);
                }
                KeyCode::Char('q') => return Ok(true),
                _ => {}
            }
            return Ok(false);
        }

        match event.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('t') | KeyCode::Esc => {
                self.toggle_tree_view();
            }
            KeyCode::Char('/') => {
                self.toggle_tree_view();
                self.set_mode(AppMode::Search);
            }
            KeyCode::Char('s') => {
                let target = self.tree_selected_pid();
                self.open_signal_menu(target);
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_tree_collapse();
            }
            KeyCode::Char('x') => self.open_tree_kill_prompt(),
            KeyCode::Char('h') => self.open_history_popup(),
            KeyCode::Char('?') => self.open_help_popup(),
            KeyCode::Char('j') => self.tree_select_next(),
            KeyCode::Char('k') => self.tree_select_prev(),
            KeyCode::Up => self.tree_select_prev(),
            KeyCode::Down => self.tree_select_next(),
            KeyCode::PageUp => {
                for _ in 0..5 {
                    self.tree_select_prev();
                }
            }
            KeyCode::PageDown => {
                for _ in 0..5 {
                    self.tree_select_next();
                }
            }
            KeyCode::Char('g') => self.tree_select_top(),
            KeyCode::Char('G') => self.tree_select_bottom(),
            _ => {}
        }

        Ok(false)
    }

    fn tree_select_next(&mut self) {
        if self.tree_rows.is_empty() {
            return;
        }
        if self.tree_selected_index + 1 < self.tree_rows.len() {
            self.tree_selected_index += 1;
        }
        self.needs_refresh = true;
    }

    fn tree_select_prev(&mut self) {
        if self.tree_rows.is_empty() {
            return;
        }
        if self.tree_selected_index > 0 {
            self.tree_selected_index -= 1;
        }
        self.needs_refresh = true;
    }

    fn tree_select_top(&mut self) {
        if self.tree_rows.is_empty() {
            return;
        }
        self.tree_selected_index = 0;
        self.needs_refresh = true;
    }

    fn tree_select_bottom(&mut self) {
        if self.tree_rows.is_empty() {
            return;
        }
        self.tree_selected_index = self.tree_rows.len() - 1;
        self.needs_refresh = true;
    }

    fn toggle_tree_collapse(&mut self) {
        if let Some(row) = self.tree_rows.get(self.tree_selected_index).cloned() {
            if !row.has_children {
                return;
            }
            if self.tree_collapsed.remove(&row.pid) {
                // expanded
            } else {
                self.tree_collapsed.insert(row.pid);
            }
            self.rebuild_tree_nodes();
            self.needs_refresh = true;
        }
    }

    fn tree_kill_preview_confirm(&mut self, confirm: bool) {
        if !confirm {
            self.tree_kill_prompt = None;
            self.needs_refresh = true;
            return;
        }

        let Some(prompt) = self.tree_kill_prompt.clone() else {
            return;
        };

        self.tree_kill_prompt = None;
        let executed =
            self.dispatch_signal_targets(vec![prompt.pid], prompt.signal, KillMode::Tree, true);
        if executed && self.tree_view_open {
            self.rebuild_tree_nodes();
        }
    }

    fn open_tree_kill_prompt(&mut self) {
        let Some(pid) = self.tree_selected_pid() else {
            return;
        };
        let lines = self.build_tree_preview_lines(pid);
        if lines.is_empty() {
            self.set_status(StatusLevel::Warning, "no processes in subtree");
            return;
        }
        self.tree_kill_prompt = Some(TreeKillPrompt {
            pid,
            signal: Signal::Sigterm,
            lines,
            risk: self.risk_for_pid(pid),
        });
        self.needs_refresh = true;
    }

    fn rebuild_tree_nodes(&mut self) {
        if !self.tree_view_open {
            return;
        }

        let processes = self.process_manager.get_processes(true);
        let map: HashMap<u32, ProcessInfo> = processes.into_iter().map(|p| (p.pid, p)).collect();

        self.tree_collapsed.retain(|pid| map.contains_key(pid));

        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

        for info in map.values() {
            let parent = info
                .parent_pid
                .filter(|pid| map.contains_key(pid))
                .unwrap_or(0);
            children.entry(parent).or_default().push(info.pid);
        }

        for list in children.values_mut() {
            list.sort_by(|a, b| {
                let proc_a = map.get(a).unwrap();
                let proc_b = map.get(b).unwrap();
                proc_b
                    .cpu_percent
                    .partial_cmp(&proc_a.cpu_percent)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| proc_a.name.cmp(&proc_b.name))
            });
        }

        let mut rows = Vec::new();

        let mut roots = children.get(&0).cloned().unwrap_or_default();
        if roots.is_empty() {
            roots = map.keys().cloned().collect();
        }

        roots.sort_by(|a, b| {
            let proc_a = map.get(a).unwrap();
            let proc_b = map.get(b).unwrap();
            proc_b
                .cpu_percent
                .partial_cmp(&proc_a.cpu_percent)
                .unwrap_or(Ordering::Equal)
                .then_with(|| proc_a.name.cmp(&proc_b.name))
        });
        roots.dedup();

        let mut branch_stack = Vec::new();
        let mut visited = HashSet::new();

        for root_pid in roots.iter() {
            branch_stack.clear();
            let _ =
                self.flatten_tree_node(*root_pid, &mut branch_stack, &map, &children, &mut rows);
            visited.insert(*root_pid);
        }

        for pid in map.keys() {
            if !visited.contains(pid) {
                branch_stack.clear();
                let _ = self.flatten_tree_node(*pid, &mut branch_stack, &map, &children, &mut rows);
            }
        }

        let previous_pid = self
            .tree_rows
            .get(self.tree_selected_index)
            .map(|row| row.pid);
        self.tree_rows = rows;

        if let Some(pid) = previous_pid {
            if let Some(idx) = self.tree_rows.iter().position(|row| row.pid == pid) {
                self.tree_selected_index = idx;
            } else {
                self.tree_selected_index = 0;
            }
        } else {
            self.tree_selected_index = 0;
        }

        if self.tree_rows.is_empty() {
            self.tree_selected_index = 0;
        }

        self.tree_scroll_offset = self
            .tree_scroll_offset
            .min(self.tree_rows.len().saturating_sub(1));
    }

    fn flatten_tree_node(
        &self,
        pid: u32,
        branch_stack: &mut Vec<bool>,
        map: &HashMap<u32, ProcessInfo>,
        children: &HashMap<u32, Vec<u32>>,
        rows: &mut Vec<TreeRow>,
    ) -> (f32, u64) {
        let Some(info) = map.get(&pid) else {
            return (0.0, 0);
        };

        let depth = branch_stack.len();
        let prefix = build_tree_prefix(branch_stack);
        let has_children = children.get(&pid).map(|v| !v.is_empty()).unwrap_or(false);
        let collapsed = self.tree_collapsed.contains(&pid);

        let mut total_cpu = info.cpu_percent;
        let mut total_mem = info.memory_bytes;
        let risk = self.assess_risk(info);

        let row_index = rows.len();
        rows.push(TreeRow {
            pid,
            parent_pid: info.parent_pid,
            name: info.name.clone(),
            cpu_percent: info.cpu_percent,
            memory_bytes: info.memory_bytes,
            subtree_cpu: info.cpu_percent,
            subtree_memory_bytes: info.memory_bytes,
            depth,
            has_children,
            collapsed,
            prefix,
            risk,
        });

        if let Some(child_list) = children.get(&pid) {
            if collapsed {
                for child_pid in child_list {
                    let (child_cpu, child_mem) = self.subtree_totals(*child_pid, map, children);
                    total_cpu += child_cpu;
                    total_mem += child_mem;
                }
            } else {
                for (idx, child_pid) in child_list.iter().enumerate() {
                    branch_stack.push(idx + 1 == child_list.len());
                    let (child_cpu, child_mem) =
                        self.flatten_tree_node(*child_pid, branch_stack, map, children, rows);
                    total_cpu += child_cpu;
                    total_mem += child_mem;
                    branch_stack.pop();
                }
            }
        }

        if let Some(row) = rows.get_mut(row_index) {
            row.subtree_cpu = total_cpu;
            row.subtree_memory_bytes = total_mem;
        }

        (total_cpu, total_mem)
    }

    fn subtree_totals(
        &self,
        pid: u32,
        map: &HashMap<u32, ProcessInfo>,
        children: &HashMap<u32, Vec<u32>>,
    ) -> (f32, u64) {
        let Some(info) = map.get(&pid) else {
            return (0.0, 0);
        };
        let mut total_cpu = info.cpu_percent;
        let mut total_mem = info.memory_bytes;
        if let Some(child_list) = children.get(&pid) {
            for child_pid in child_list {
                let (child_cpu, child_mem) = self.subtree_totals(*child_pid, map, children);
                total_cpu += child_cpu;
                total_mem += child_mem;
            }
        }
        (total_cpu, total_mem)
    }

    fn build_tree_preview_lines(&mut self, pid: u32) -> Vec<String> {
        let mut processes = self.process_manager.get_process_tree(pid);
        if processes.is_empty() {
            processes = get_process_tree(pid);
        }
        if processes.is_empty() {
            return Vec::new();
        }

        let map: HashMap<u32, ProcessInfo> =
            processes.into_iter().map(|proc| (proc.pid, proc)).collect();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

        for info in map.values() {
            if let Some(parent) = info.parent_pid {
                if map.contains_key(&parent) {
                    children.entry(parent).or_default().push(info.pid);
                }
            }
        }

        for list in children.values_mut() {
            list.sort_by(|a, b| {
                let proc_a = map.get(a).unwrap();
                let proc_b = map.get(b).unwrap();
                proc_b
                    .cpu_percent
                    .partial_cmp(&proc_a.cpu_percent)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| proc_a.name.cmp(&proc_b.name))
            });
        }

        let mut lines = Vec::new();
        let mut stack = Vec::new();
        self.build_preview_recursive(pid, &mut stack, &map, &children, &mut lines);
        lines
    }

    fn build_preview_recursive(
        &self,
        pid: u32,
        stack: &mut Vec<bool>,
        map: &HashMap<u32, ProcessInfo>,
        children: &HashMap<u32, Vec<u32>>,
        lines: &mut Vec<String>,
    ) {
        let Some(info) = map.get(&pid) else {
            return;
        };

        let prefix = build_tree_prefix(stack);
        let mut line = format!(
            "{}[{}] {} [CPU: {:>5.1}%] [MEM: {}]",
            prefix,
            info.pid,
            info.name,
            info.cpu_percent,
            format_bytes(info.memory_bytes)
        );
        if let Some(risk) = self.assess_risk(info) {
            let label = match risk.level {
                RiskLevel::Critical => "CRITICAL",
                RiskLevel::Elevated => "warn",
            };
            line.push_str(&format!(" [{}: {}]", label, risk.reason));
        }
        lines.push(line);

        if let Some(child_list) = children.get(&pid) {
            for (idx, child_pid) in child_list.iter().enumerate() {
                stack.push(idx + 1 == child_list.len());
                self.build_preview_recursive(*child_pid, stack, map, children, lines);
                stack.pop();
            }
        }
    }

    fn handle_normal_input(&mut self, event: KeyEvent) -> Result<bool> {
        match event.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Esc => {
                if self.is_info_pane_open() {
                    self.toggle_info_pane();
                } else {
                    self.set_status(StatusLevel::Info, "Press q to quit or ? for help");
                    self.needs_refresh = true;
                }
            }
            KeyCode::Char('/') => {
                self.set_mode(AppMode::Search);
                self.set_status(
                    StatusLevel::Info,
                    "Search mode: type to filter, Enter/Esc to exit".to_string(),
                );
                self.needs_refresh = true;
            }
            KeyCode::Char('i') => {
                self.toggle_info_pane();
            }
            KeyCode::Tab => {
                if self.is_info_pane_open() {
                    self.toggle_info_focus();
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') if self.is_info_pane_open() => {
                self.toggle_info_env();
            }
            KeyCode::Char('f') | KeyCode::Char('F') if self.is_info_pane_open() => {
                self.toggle_info_files();
            }
            KeyCode::Char('m') | KeyCode::Char('M') if self.is_info_pane_open() => {
                self.toggle_info_maps();
            }
            KeyCode::Char('n') | KeyCode::Char('N') if self.is_info_pane_open() => {
                self.toggle_info_network();
            }
            KeyCode::Char('c') | KeyCode::Char('C') if self.is_info_pane_open() => {
                self.toggle_info_cgroups();
            }
            KeyCode::Char('t') => {
                self.toggle_tree_view();
            }
            KeyCode::Char('s') => {
                let target = if self.tree_view_open {
                    self.tree_selected_pid()
                } else {
                    self.current_pid()
                };
                self.open_signal_menu(target);
            }
            KeyCode::Char('h') => {
                self.open_history_popup();
            }
            KeyCode::Char('x') => self.kill_selected_with_tree(Signal::Sigterm),
            KeyCode::Char('k') if self.is_info_pane_open() && self.info_focus() => {
                self.scroll_info_pane(-1);
            }
            KeyCode::Char('j') => {
                if self.is_info_pane_open() && self.info_focus() {
                    self.scroll_info_pane(1);
                } else {
                    self.select_next();
                }
            }
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
                self.open_help_popup();
            }
            KeyCode::Char(' ') => self.toggle_selection(),
            KeyCode::Enter => self.kill_selected(Signal::Sigterm),
            KeyCode::Up => {
                if self.is_info_pane_open() && self.info_focus() {
                    self.scroll_info_pane(-1);
                } else {
                    self.select_prev();
                }
            }
            KeyCode::Down => {
                if self.is_info_pane_open() && self.info_focus() {
                    self.scroll_info_pane(1);
                } else {
                    self.select_next();
                }
            }
            KeyCode::PageUp => {
                if self.is_info_pane_open() && self.info_focus() {
                    self.scroll_info_pane(-5);
                }
            }
            KeyCode::PageDown => {
                if self.is_info_pane_open() && self.info_focus() {
                    self.scroll_info_pane(5);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_search_input(&mut self, event: KeyEvent) -> Result<bool> {
        match event.code {
            KeyCode::Esc => {
                self.flush_search_filters();
                self.set_mode(AppMode::Normal);
            }
            KeyCode::Enter => {
                self.flush_search_filters();
                self.set_mode(AppMode::Normal);
            }
            KeyCode::Backspace => {
                if self.search_query.pop().is_some() {
                    self.mark_search_dirty();
                } else {
                    self.needs_refresh = true;
                }
            }
            KeyCode::Char(c)
                if !event.modifiers.contains(KeyModifiers::CONTROL)
                    && !event.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.search_query.push(c);
                self.mark_search_dirty();
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_signal_menu_input(&mut self, event: KeyEvent) -> Result<bool> {
        let signals = Signal::all();
        if signals.is_empty() {
            self.close_signal_menu();
            return Ok(false);
        }

        match event.code {
            KeyCode::Esc => {
                self.close_signal_menu();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.signal_menu_selected == 0 {
                    self.signal_menu_selected = signals.len() - 1;
                } else {
                    self.signal_menu_selected -= 1;
                }
                self.needs_refresh = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.signal_menu_selected = (self.signal_menu_selected + 1) % signals.len();
                self.needs_refresh = true;
            }
            KeyCode::Enter => {
                let index = self.signal_menu_selected.min(signals.len() - 1);
                let signal = signals[index];
                self.send_signal_from_menu(signal);
            }
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let digit = c.to_digit(10).unwrap() as i32;
                if let Some(idx) = signals.iter().position(|sig| sig.number() == digit) {
                    self.signal_menu_selected = idx;
                    self.needs_refresh = true;
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_shell_confirm_input(&mut self, event: KeyEvent) -> Result<Option<bool>> {
        if self.shell_confirm.is_none() {
            return Ok(None);
        }

        match event.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(pending) = self.shell_confirm.take() {
                    match pending {
                        PendingKill::Direct { targets, signal } => {
                            self.dispatch_signal_targets(targets, signal, KillMode::Direct, true);
                        }
                        PendingKill::Tree { targets, signal } => {
                            self.dispatch_signal_targets(targets, signal, KillMode::Tree, true);
                        }
                    }
                }
                self.refresh_pause_state();
                Ok(Some(false))
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.shell_confirm = None;
                self.set_status(StatusLevel::Info, "cancelled shell kill".to_string());
                self.needs_refresh = true;
                self.refresh_pause_state();
                Ok(Some(false))
            }
            _ => {
                self.set_status(
                    StatusLevel::Warning,
                    "Press y to continue or n to cancel".to_string(),
                );
                self.needs_refresh = true;
                Ok(Some(false))
            }
        }
    }

    fn refresh_process_data(&mut self) {
        self.processes = self.process_manager.get_processes(self.show_all_processes);
        self.total_memory_bytes = self.process_manager.total_memory_bytes();
        self.selected_pids
            .retain(|pid| self.processes.iter().any(|proc| proc.pid == *pid));
        self.apply_filters();
        if self.tree_view_open {
            self.rebuild_tree_nodes();
        }
    }

    fn force_refresh_processes(&mut self) {
        let paused = self.paused;
        self.paused = false;
        self.refresh_process_data();
        self.paused = paused;
    }

    fn invalidate_process_details(&mut self) {
        self.info_details_cache = None;
        self.info_pane_scroll = 0;
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
            self.table_scroll_offset = 0;
        } else if self.selected_index >= self.filtered_processes.len() {
            self.selected_index = self.filtered_processes.len() - 1;
        }

        if self.selected_index < self.table_scroll_offset {
            self.table_scroll_offset = self.selected_index;
        }

        if let Some(last) = self.filtered_processes.len().checked_sub(1) {
            if self.table_scroll_offset > last {
                self.table_scroll_offset = last;
            }
        }
    }

    pub fn current_pid(&self) -> Option<u32> {
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
        self.paused = matches!(self.mode, AppMode::Search | AppMode::SignalMenu)
            || self.history_popup_open
            || self.help_popup_open
            || self.shell_confirm.is_some();
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

    fn parse_search_mode(query: &str) -> Result<SearchMode, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(SearchMode::Fuzzy(String::new()));
        }

        let lowered = trimmed.to_ascii_lowercase();
        if lowered.starts_with("/killed") {
            let remainder = if lowered.len() >= 7 {
                &trimmed[7..]
            } else {
                ""
            };
            let filter = remainder.trim_start_matches([' ', ':']);
            return Ok(SearchMode::History(filter.to_string()));
        }

        if trimmed.starts_with('/') {
            if let Some(end) = trimmed.rfind('/') {
                if end > 0 {
                    let pattern = &trimmed[1..end];
                    let flags = trimmed[end + 1..].to_string();
                    let mut builder = RegexBuilder::new(pattern);
                    if flags.contains('i') {
                        builder.case_insensitive(true);
                    }
                    if flags.contains('m') {
                        builder.multi_line(true);
                    }
                    if flags.contains('s') {
                        builder.dot_matches_new_line(true);
                    }
                    let matcher = builder
                        .build()
                        .map_err(|err| format!("invalid regex: {err}"))?;
                    return Ok(SearchMode::Regex {
                        pattern: pattern.to_string(),
                        flags,
                        matcher,
                    });
                }
            }
        }

        Ok(SearchMode::Fuzzy(trimmed.to_string()))
    }

    fn filter_by_history(&mut self, processes: Vec<ProcessInfo>, filter: &str) -> Vec<ProcessInfo> {
        const HISTORY_WEIGHT: i64 = 1_000_000_000;
        let filter_norm = filter.trim().to_ascii_lowercase();
        let mut matched: HashMap<u32, usize> = HashMap::new();

        for (idx, event) in self.signal_sender.history().enumerate() {
            if event.result.is_err() {
                continue;
            }
            if !filter_norm.is_empty() {
                let signal_name = event.signal.name().to_ascii_lowercase();
                let proc_name = event.process_name.to_ascii_lowercase();
                if !signal_name.contains(&filter_norm) && !proc_name.contains(&filter_norm) {
                    continue;
                }
            }
            matched.entry(event.pid).or_insert(idx);
        }

        processes
            .into_iter()
            .filter_map(|proc| {
                matched.get(&proc.pid).map(|order| {
                    let highlights = full_match_indices(&proc.name);
                    if !highlights.is_empty() {
                        self.search_matches.insert(proc.pid, highlights);
                    }
                    let score = HISTORY_WEIGHT - (*order as i64);
                    self.search_scores.insert(proc.pid, score);
                    proc
                })
            })
            .collect()
    }

    fn process_snapshot(&self, pid: u32) -> Option<ProcessInfo> {
        self.processes
            .iter()
            .find(|proc| proc.pid == pid)
            .cloned()
            .or_else(|| {
                self.filtered_processes
                    .iter()
                    .find(|proc| proc.pid == pid)
                    .cloned()
            })
    }

    fn risk_for_pid(&self, pid: u32) -> Option<RiskInfo> {
        if let Some(info) = self.process_snapshot(pid) {
            return self.assess_risk(&info);
        }
        self.tree_rows
            .iter()
            .find(|row| row.pid == pid)
            .and_then(|row| row.risk.clone())
    }

    fn assess_risk(&self, info: &ProcessInfo) -> Option<RiskInfo> {
        if info.pid == 1 {
            return Some(RiskInfo {
                level: RiskLevel::Critical,
                reason: "init process".to_string(),
            });
        }
        if info.pid == self.parent_pid {
            return Some(RiskInfo {
                level: RiskLevel::Critical,
                reason: "current shell".to_string(),
            });
        }

        let name = info.name.to_ascii_lowercase();
        let mut result: Option<RiskInfo> = None;

        for (pattern, level, reason) in CRITICAL_NAME_PATTERNS.iter() {
            if name.contains(pattern) {
                result = combine_risk(result, *level, reason);
            }
        }

        if info.user == "root" {
            result = combine_risk(result, RiskLevel::Elevated, "root-owned process");
        }

        result
    }
}

fn order_text(desc: bool) -> &'static str {
    if desc { "(desc)" } else { "(asc)" }
}

fn build_tree_prefix(stack: &[bool]) -> String {
    if stack.is_empty() {
        return String::new();
    }

    let mut prefix = String::new();
    for (idx, is_last) in stack.iter().enumerate() {
        if idx + 1 == stack.len() {
            prefix.push_str(if *is_last { "└─ " } else { "├─ " });
        } else {
            prefix.push_str(if *is_last { "   " } else { "│  " });
        }
    }
    prefix
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

const CRITICAL_NAME_PATTERNS: &[(&str, RiskLevel, &str)] = &[
    ("systemd", RiskLevel::Critical, "system init"),
    ("dbus-daemon", RiskLevel::Elevated, "dbus session"),
    ("dbus-broker", RiskLevel::Elevated, "dbus broker"),
    ("gnome-shell", RiskLevel::Critical, "desktop shell"),
    ("plasmashell", RiskLevel::Critical, "desktop shell"),
    ("kwin", RiskLevel::Critical, "window manager"),
    ("mutter", RiskLevel::Critical, "window manager"),
    ("sway", RiskLevel::Critical, "window manager"),
    ("hyprland", RiskLevel::Critical, "window manager"),
    ("wayfire", RiskLevel::Critical, "window manager"),
    ("i3", RiskLevel::Critical, "window manager"),
    ("xfce4-session", RiskLevel::Elevated, "desktop session"),
    ("xorg", RiskLevel::Critical, "display server"),
    ("xwayland", RiskLevel::Elevated, "display bridge"),
    ("pipewire", RiskLevel::Elevated, "media service"),
    ("pulseaudio", RiskLevel::Elevated, "audio server"),
    ("tmux", RiskLevel::Elevated, "terminal multiplexer"),
    ("wezterm", RiskLevel::Elevated, "terminal host"),
    ("alacritty", RiskLevel::Elevated, "terminal host"),
    ("kitty", RiskLevel::Elevated, "terminal host"),
];

fn combine_risk(current: Option<RiskInfo>, level: RiskLevel, reason: &str) -> Option<RiskInfo> {
    match current {
        Some(existing) if existing.level >= level => Some(existing),
        _ => Some(RiskInfo {
            level,
            reason: reason.to_string(),
        }),
    }
}

const SCORE_NAME: i64 = 900_000;
const SCORE_CAMEL: i64 = 880_000;
const SCORE_CMDLINE: i64 = 700_000;
const SCORE_CWD: i64 = 660_000;
const SCORE_ENV: i64 = 640_000;
const MAX_ENV_MATCHES: usize = 16;

fn fuzzy_match_process(
    proc: &ProcessInfo,
    query: &str,
    matcher: &SkimMatcherV2,
) -> Option<SearchHit> {
    let mut best_score: Option<i64> = None;
    let mut name_indices: Vec<usize> = Vec::new();

    if let Some((score, indices)) = matcher.fuzzy_indices(&proc.name, query) {
        let weighted = SCORE_NAME + score;
        best_score = Some(weighted);
        name_indices = indices;
    }

    let camel = split_camel_case(&proc.name);
    if !camel.is_empty() {
        if let Some(score) = matcher.fuzzy_match(&camel, query) {
            let weighted = SCORE_CAMEL + score;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    if !proc.cmdline.is_empty() {
        let cmdline = proc.cmdline.join(" ");
        if let Some(score) = matcher.fuzzy_match(&cmdline, query) {
            let weighted = SCORE_CMDLINE + score;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    if let Some(cwd) = proc.cwd.as_ref() {
        if let Some(score) = matcher.fuzzy_match(cwd, query) {
            let weighted = SCORE_CWD + score;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    for entry in proc.environment.iter().take(MAX_ENV_MATCHES) {
        if let Some(score) = matcher.fuzzy_match(entry, query) {
            let weighted = SCORE_ENV + score;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    best_score.map(|score| SearchHit {
        score,
        name_indices,
    })
}

fn regex_match_process(proc: &ProcessInfo, regex: &Regex) -> Option<SearchHit> {
    let mut best_score: Option<i64> = None;
    let mut name_indices: Vec<usize> = Vec::new();

    if regex.is_match(&proc.name) {
        name_indices = regex_indices(&proc.name, regex);
        let weighted = SCORE_NAME + name_indices.len() as i64;
        best_score = Some(weighted);
    }

    if !proc.cmdline.is_empty() {
        let cmdline = proc.cmdline.join(" ");
        if regex.is_match(&cmdline) {
            let weighted = SCORE_CMDLINE + cmdline.len() as i64;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    if let Some(cwd) = proc.cwd.as_ref() {
        if regex.is_match(cwd) {
            let weighted = SCORE_CWD + cwd.len() as i64;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    for entry in proc.environment.iter().take(MAX_ENV_MATCHES) {
        if regex.is_match(entry) {
            let weighted = SCORE_ENV + entry.len() as i64;
            if best_score.map_or(true, |current| weighted > current) {
                best_score = Some(weighted);
            }
        }
    }

    best_score.map(|score| SearchHit {
        score,
        name_indices,
    })
}

fn regex_indices(text: &str, regex: &Regex) -> Vec<usize> {
    let mut indices = Vec::new();
    for mat in regex.find_iter(text) {
        let start = mat.start();
        let slice = &text[start..mat.end()];
        for (offset, _) in slice.char_indices() {
            indices.push(start + offset);
        }
    }
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn full_match_indices(text: &str) -> Vec<usize> {
    text.char_indices().map(|(idx, _)| idx).collect()
}

fn split_camel_case(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(value.len() * 2);
    let mut prev_lower_or_digit = false;

    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            result.push(' ');
            prev_lower_or_digit = false;
            continue;
        }

        if ch.is_uppercase() && prev_lower_or_digit {
            result.push(' ');
        }

        result.push(ch);
        prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
    }

    result
}

fn is_dangerous_signal(signal: Signal) -> bool {
    matches!(
        signal,
        Signal::Sigkill
            | Signal::Sigstop
            | Signal::Sigabrt
            | Signal::Sigbus
            | Signal::Sigfpe
            | Signal::Sigill
            | Signal::Sigsegv
            | Signal::Sigtrap
            | Signal::Sigsys
    )
}
