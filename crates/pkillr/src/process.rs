use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nix::unistd::{Uid as NixUid, User};
use sysinfo::{
    MINIMUM_CPU_UPDATE_INTERVAL, Pid, Process, ProcessRefreshKind, ProcessStatus, RefreshKind,
    System,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Stopped,
    Zombie,
    Idle,
    Dead,
    Tracing,
    DiskSleep,
    Locked,
    Waking,
    Parked,
    Unknown,
}

impl From<ProcessStatus> for ProcessState {
    fn from(status: ProcessStatus) -> Self {
        match status {
            ProcessStatus::Run => ProcessState::Running,
            ProcessStatus::Sleep => ProcessState::Sleeping,
            ProcessStatus::Stop => ProcessState::Stopped,
            ProcessStatus::Zombie => ProcessState::Zombie,
            ProcessStatus::Idle => ProcessState::Idle,
            ProcessStatus::Dead => ProcessState::Dead,
            ProcessStatus::Tracing => ProcessState::Tracing,
            ProcessStatus::UninterruptibleDiskSleep => ProcessState::DiskSleep,
            ProcessStatus::LockBlocked => ProcessState::Locked,
            ProcessStatus::Waking | ProcessStatus::Wakekill => ProcessState::Waking,
            ProcessStatus::Parked => ProcessState::Parked,
            ProcessStatus::Unknown(_) => ProcessState::Unknown,
        }
    }
}

impl ProcessState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProcessState::Running => "Running",
            ProcessState::Sleeping => "Sleeping",
            ProcessState::Stopped => "Stopped",
            ProcessState::Zombie => "Zombie",
            ProcessState::Idle => "Idle",
            ProcessState::Dead => "Dead",
            ProcessState::Tracing => "Tracing",
            ProcessState::DiskSleep => "Disk Sleep",
            ProcessState::Locked => "Locked",
            ProcessState::Waking => "Waking",
            ProcessState::Parked => "Parked",
            ProcessState::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub user: String,
    pub runtime: Duration,
    pub cmdline: Vec<String>,
    pub cwd: Option<String>,
    pub environment: Vec<String>,
    pub parent_pid: Option<u32>,
    pub state: ProcessState,
}

#[derive(Debug, Clone)]
pub struct ChildProcess {
    pub pid: u32,
    pub name: String,
    pub state: ProcessState,
}

#[derive(Debug, Clone)]
pub struct ProcessDetails {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub state: ProcessState,
    pub thread_count: usize,
    pub cmdline: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub environment: Vec<String>,
    pub children: Vec<ChildProcess>,
    pub capabilities: Vec<String>,
    pub open_files: Vec<String>,
    pub open_ports: Vec<String>,
    pub cgroups: Vec<String>,
    pub namespaces: Vec<String>,
    pub memory_maps: Vec<String>,
}

pub struct ProcessManager {
    system: System,
    cpu_cache: HashMap<u32, f32>,
    username_cache: HashMap<u32, String>,
    last_refresh: Instant,
    process_refresh: ProcessRefreshKind,
}

impl ProcessManager {
    pub fn new() -> Self {
        let process_refresh = ProcessRefreshKind::everything();
        let system = System::new_with_specifics(RefreshKind::new().with_processes(process_refresh));
        let mut manager = Self {
            system,
            cpu_cache: HashMap::new(),
            username_cache: HashMap::new(),
            last_refresh: Instant::now() - MINIMUM_CPU_UPDATE_INTERVAL,
            process_refresh,
        };
        manager.force_refresh();
        manager
    }

    pub fn get_processes(&mut self, show_all: bool) -> Vec<ProcessInfo> {
        let refreshed = self.refresh_if_needed();
        let current_uid = NixUid::current();
        let mut results = Vec::new();
        let mut seen = HashSet::new();

        let pids: Vec<Pid> = self.system.processes().keys().copied().collect();

        for pid in pids {
            if let Some(process) = self.system.process(pid) {
                let pid_u32 = pid.as_u32();
                if !show_all && !visible_to_user(process, current_uid) {
                    continue;
                }

                let snapshot = {
                    let cpu_sample = normalize_cpu(process.cpu_usage());
                    let memory_bytes = process.memory().saturating_mul(1_024);
                    let runtime = Duration::from_secs(process.run_time());
                    let cmdline = process.cmd().to_vec();
                    let cwd = process
                        .cwd()
                        .map(|path| path.to_string_lossy().into_owned());
                    let environment = process.environ().to_vec();
                    let parent_pid = process.parent().map(|p| p.as_u32());
                    let state = ProcessState::from(process.status());
                    let name = process.name().to_string();
                    let user_uid = process.user_id().map(|uid| raw_uid(uid));
                    (
                        cpu_sample,
                        memory_bytes,
                        runtime,
                        cmdline,
                        cwd,
                        environment,
                        parent_pid,
                        state,
                        name,
                        user_uid,
                    )
                };

                let (
                    cpu_sample,
                    memory_bytes,
                    runtime,
                    cmdline,
                    cwd,
                    environment,
                    parent_pid,
                    state,
                    name,
                    user_uid,
                ) = snapshot;

                let cpu_percent = self.cpu_percent(pid_u32, cpu_sample, refreshed);
                let user = user_uid
                    .map(|uid| self.username_from_uid(uid))
                    .unwrap_or_else(|| "unknown".to_string());

                let info = ProcessInfo {
                    pid: pid_u32,
                    name,
                    cpu_percent,
                    memory_bytes,
                    user,
                    runtime,
                    cmdline,
                    cwd,
                    environment,
                    parent_pid,
                    state,
                };

                seen.insert(pid_u32);
                results.push(info);
            }
        }

        self.cpu_cache.retain(|pid, _| seen.contains(pid));
        results
    }

    pub fn get_process_tree(&mut self, pid: u32) -> Vec<ProcessInfo> {
        let processes = self.get_processes(true);
        let mut by_pid: HashMap<u32, ProcessInfo> =
            processes.into_iter().map(|info| (info.pid, info)).collect();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

        for (child_pid, info) in &by_pid {
            if let Some(parent) = info.parent_pid {
                children.entry(parent).or_default().push(*child_pid);
            }
        }

        let mut stack = vec![pid];
        let mut tree = Vec::new();
        while let Some(current) = stack.pop() {
            if let Some(info) = by_pid.remove(&current) {
                if let Some(kids) = children.get(&current) {
                    for child in kids.iter().rev() {
                        stack.push(*child);
                    }
                }
                tree.push(info);
            }
        }

        tree
    }

    pub fn get_details(&mut self, pid: u32) -> Option<ProcessDetails> {
        let sys_pid = Pid::from_u32(pid);
        self.system.refresh_process(sys_pid);
        let process = self.system.process(sys_pid)?;

        let parent_pid = process.parent().map(|p| p.as_u32());
        let state = ProcessState::from(process.status());
        let thread_count = process.tasks().map(|tasks| tasks.len()).unwrap_or(1);
        let cmdline = process.cmd().to_vec();
        let cwd = process.cwd().map(|path| path.to_path_buf());
        let environment = process.environ().to_vec();

        let children = self
            .system
            .processes()
            .iter()
            .filter_map(|(child_pid, child)| {
                if child.parent() == Some(sys_pid) {
                    Some(ChildProcess {
                        pid: child_pid.as_u32(),
                        name: child.name().to_string(),
                        state: ProcessState::from(child.status()),
                    })
                } else {
                    None
                }
            })
            .collect();

        let capabilities = read_capabilities(pid);
        let open_files = read_open_files(pid);
        let open_ports = read_open_ports(pid);
        let cgroups = read_cgroups(pid);
        let namespaces = read_namespaces(pid);
        let memory_maps = read_memory_maps(pid);

        Some(ProcessDetails {
            pid,
            parent_pid,
            state,
            thread_count,
            cmdline,
            cwd,
            environment,
            children,
            capabilities,
            open_files,
            open_ports,
            cgroups,
            namespaces,
            memory_maps,
        })
    }

    fn refresh_if_needed(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_refresh) >= MINIMUM_CPU_UPDATE_INTERVAL {
            self.force_refresh();
            true
        } else {
            false
        }
    }

    fn force_refresh(&mut self) {
        self.system
            .refresh_processes_specifics(self.process_refresh);
        self.last_refresh = Instant::now();
    }

    fn cpu_percent(&mut self, pid: u32, sample: f32, refreshed: bool) -> f32 {
        if refreshed {
            self.cpu_cache.insert(pid, sample);
            sample
        } else if let Some(value) = self.cpu_cache.get(&pid).copied() {
            value
        } else {
            self.cpu_cache.insert(pid, sample);
            sample
        }
    }

    fn username_from_uid(&mut self, uid: u32) -> String {
        if let Some(name) = self.username_cache.get(&uid) {
            return name.clone();
        }

        let lookup = User::from_uid(NixUid::from_raw(uid))
            .ok()
            .flatten()
            .map(|user| user.name);
        let name = lookup.unwrap_or_else(|| "unknown".to_string());

        self.username_cache.insert(uid, name.clone());
        name
    }

    pub fn total_memory_bytes(&self) -> u64 {
        self.system.total_memory() * 1_024
    }
}

fn raw_uid(uid: &sysinfo::Uid) -> u32 {
    (**uid) as u32
}

fn normalize_cpu(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(target_os = "linux")]
fn read_capabilities(pid: u32) -> Vec<String> {
    let path = format!("/proc/{pid}/status");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    reader
        .lines()
        .filter_map(|line| line.ok())
        .filter(|line| line.starts_with("Cap"))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn read_capabilities(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_open_files(pid: u32) -> Vec<String> {
    let mut result = Vec::new();
    let path = format!("/proc/{pid}/fd");
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let fd = entry
            .file_name()
            .into_string()
            .unwrap_or_else(|_| "?".to_string());
        let target = fs::read_link(entry.path())
            .map(|link| link.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "<permission denied>".to_string());
        result.push(format!("fd {fd} -> {target}"));
    }

    result.sort();
    result
}

#[cfg(not(target_os = "linux"))]
fn read_open_files(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_open_ports(pid: u32) -> Vec<String> {
    let mut entries = Vec::new();
    for table in ["tcp", "tcp6"] {
        let path = format!("/proc/{pid}/net/{table}");
        if let Ok(file) = fs::File::open(path) {
            for (index, line) in BufReader::new(file).lines().enumerate() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => continue,
                };
                if index == 0 || line.trim().is_empty() {
                    continue;
                }
                if let Some(parsed) = parse_tcp_line(&line) {
                    entries.push(format!("{table}: {parsed}"));
                }
            }
        }
    }
    entries
}

#[cfg(not(target_os = "linux"))]
fn read_open_ports(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn parse_tcp_line(line: &str) -> Option<String> {
    let columns: Vec<&str> = line.split_whitespace().collect();
    if columns.len() < 4 {
        return None;
    }
    let local = columns[1];
    let remote = columns[2];
    let state = tcp_state_name(columns[3]);
    Some(format!("{local} -> {remote} ({state})"))
}

#[cfg(target_os = "linux")]
fn tcp_state_name(code: &str) -> &'static str {
    match code {
        "01" => "ESTABLISHED",
        "02" => "SYN-SENT",
        "03" => "SYN-RECEIVED",
        "04" => "FIN-WAIT-1",
        "05" => "FIN-WAIT-2",
        "06" => "TIME-WAIT",
        "07" => "CLOSE",
        "08" => "CLOSE-WAIT",
        "09" => "LAST-ACK",
        "0A" => "LISTEN",
        "0B" => "CLOSING",
        "0C" => "NEW-SYN-RECV",
        _ => "UNKNOWN",
    }
}

#[cfg(not(target_os = "linux"))]
fn tcp_state_name(_code: &str) -> &'static str {
    "UNKNOWN"
}

#[cfg(target_os = "linux")]
fn read_cgroups(pid: u32) -> Vec<String> {
    let path = format!("/proc/{pid}/cgroup");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    BufReader::new(file)
        .lines()
        .filter_map(|line| line.ok())
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn read_cgroups(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_namespaces(pid: u32) -> Vec<String> {
    let mut entries = Vec::new();
    let path = format!("/proc/{pid}/ns");
    let dir = match fs::read_dir(path) {
        Ok(dir) => dir,
        Err(_) => return entries,
    };
    for entry in dir.flatten() {
        let name = entry
            .file_name()
            .into_string()
            .unwrap_or_else(|_| "?".to_string());
        let target = fs::read_link(entry.path())
            .map(|link| link.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "<permission denied>".to_string());
        entries.push(format!("{name}: {target}"));
    }
    entries.sort();
    entries
}

#[cfg(not(target_os = "linux"))]
fn read_namespaces(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_memory_maps(pid: u32) -> Vec<String> {
    const MAP_LIMIT: usize = 64;
    let path = format!("/proc/{pid}/maps");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let mut lines: Vec<String> = BufReader::new(file)
        .lines()
        .filter_map(|line| line.ok())
        .take(MAP_LIMIT)
        .collect();
    if lines.len() == MAP_LIMIT {
        lines.push("...".to_string());
    }
    lines
}

#[cfg(not(target_os = "linux"))]
fn read_memory_maps(_pid: u32) -> Vec<String> {
    Vec::new()
}

fn visible_to_user(process: &Process, current_uid: NixUid) -> bool {
    let Some(uid) = process.user_id() else {
        return false;
    };
    let raw = raw_uid(uid);
    raw == current_uid.as_raw()
}

pub fn is_system_process(proc: &ProcessInfo) -> bool {
    proc.pid <= 1 || proc.user == "root" || proc.parent_pid.is_none()
}

pub fn can_kill(proc: &ProcessInfo) -> Result<(), String> {
    if proc.pid == 1 {
        return Err("cannot kill pid 1".to_string());
    }

    if proc.pid == std::process::id() {
        return Err("cannot kill pkillr itself".to_string());
    }

    let current_uid = NixUid::current();
    if current_uid.as_raw() == 0 {
        return Ok(());
    }

    if proc.user == "unknown" {
        return Err("cannot determine process owner".to_string());
    }

    let current_user = User::from_uid(current_uid)
        .ok()
        .flatten()
        .map(|user| user.name)
        .ok_or_else(|| "cannot determine current user".to_string())?;

    if proc.user != current_user {
        return Err("insufficient permissions".to_string());
    }

    Ok(())
}

pub fn get_process_tree(pid: u32) -> Vec<ProcessInfo> {
    let mut manager = ProcessManager::new();
    manager.get_process_tree(pid)
}
