use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nix::unistd::{Uid, User};

use crate::config::{Config, SortField, Theme};
use crate::process::{ProcessDetails, ProcessInfo, ProcessManager, matches_search};
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
}

#[derive(Debug, Clone)]
pub struct TreeKillPrompt {
    pub pid: u32,
    pub signal: Signal,
    pub lines: Vec<String>,
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
    info_network_expanded: bool,
    info_cgroups_expanded: bool,
    info_details_cache: Option<(u32, ProcessDetails)>,

    table_scroll_offset: usize,
    tree_selected_index: usize,
    tree_rows: Vec<TreeRow>,
    tree_collapsed: HashSet<u32>,
    tree_scroll_offset: usize,
    tree_kill_prompt: Option<TreeKillPrompt>,
    current_username: String,
    is_root: bool,
    total_memory_bytes: u64,

    process_manager: ProcessManager,
    signal_sender: SignalSender,
}

impl App {
    pub fn new(config: Config) -> Self {
        let current_uid = Uid::current();
        let is_root = current_uid.as_raw() == 0;
        let current_username = User::from_uid(current_uid)
            .ok()
            .flatten()
            .map(|user| user.name)
            .unwrap_or_else(|| "unknown".to_string());

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
            info_network_expanded: false,
            info_cgroups_expanded: false,
            info_details_cache: None,
            table_scroll_offset: 0,
            tree_selected_index: 0,
            tree_rows: Vec::new(),
            tree_collapsed: HashSet::new(),
            tree_scroll_offset: 0,
            tree_kill_prompt: None,
            current_username,
            is_root,
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
        let query = self.search_query.trim();
        if !query.is_empty() {
            data.retain(|proc| matches_search(proc, query));
        }

        data.sort_by(|a, b| self.compare_processes(a, b));
        self.filtered_processes = data;
        self.selected_pids
            .retain(|pid| self.filtered_processes.iter().any(|proc| proc.pid == *pid));
        self.clamp_selection();
        if self.filtered_processes.is_empty() {
            self.table_scroll_offset = 0;
        } else {
            let max_offset = self.filtered_processes.len().saturating_sub(1);
            self.table_scroll_offset = self.table_scroll_offset.min(max_offset);
        }
        self.invalidate_process_details();
        self.needs_refresh = true;
    }

    pub fn handle_input(&mut self, event: KeyEvent) -> Result<bool> {
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

        self.invalidate_process_details();
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

        self.invalidate_process_details();
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
        if proc.pid == 1 || proc.pid == std::process::id() {
            return false;
        }
        if self.is_root {
            return true;
        }
        proc.user == self.current_username
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
        self.needs_refresh = true;
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

        let name = self
            .process_name_for_pid(pid)
            .unwrap_or_else(|| format!("PID {pid}"));

        let result = self.signal_sender.send_signal(pid, signal);
        self.close_signal_menu();

        match result {
            Ok(()) => {
                let message = format!("sent {} to {} (PID {pid})", signal.name(), name);
                self.set_status(StatusLevel::Info, message);
                self.force_refresh_processes();
                self.update_signal_history();
            }
            Err(err) => {
                self.set_status(StatusLevel::Error, err);
            }
        }
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

        match self
            .signal_sender
            .kill_process_tree(prompt.pid, prompt.signal)
        {
            Ok(killed) => {
                let message = format!(
                    "sent {} to {} process(es)",
                    prompt.signal.name(),
                    killed.len()
                );
                self.set_status(StatusLevel::Info, message);
                self.force_refresh_processes();
                if self.tree_view_open {
                    self.rebuild_tree_nodes();
                }
            }
            Err(err) => {
                self.set_status(StatusLevel::Error, err);
            }
        }

        self.update_signal_history();
        self.needs_refresh = true;
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
        let processes = self.process_manager.get_process_tree(pid);
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
        let line = format!(
            "{}[{}] {} [CPU: {:>5.1}%] [MEM: {}]",
            prefix,
            info.pid,
            info.name,
            info.cpu_percent,
            format_bytes(info.memory_bytes)
        );
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
            KeyCode::Char('/') => {
                self.set_mode(AppMode::Search);
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
            KeyCode::Char('e') if self.is_info_pane_open() => {
                self.toggle_info_env();
            }
            KeyCode::Char('f') if self.is_info_pane_open() => {
                self.toggle_info_files();
            }
            KeyCode::Char('n') if self.is_info_pane_open() => {
                self.toggle_info_network();
            }
            KeyCode::Char('c') if self.is_info_pane_open() => {
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
