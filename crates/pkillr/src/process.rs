use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
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

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub user: String,
    pub runtime: Duration,
    pub cmdline: Vec<String>,
    pub parent_pid: Option<u32>,
    pub state: ProcessState,
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
                    let parent_pid = process.parent().map(|p| p.as_u32());
                    let state = ProcessState::from(process.status());
                    let name = process.name().to_string();
                    let user_uid = process.user_id().map(|uid| raw_uid(uid));
                    (
                        cpu_sample,
                        memory_bytes,
                        runtime,
                        cmdline,
                        parent_pid,
                        state,
                        name,
                        user_uid,
                    )
                };

                let (cpu_sample, memory_bytes, runtime, cmdline, parent_pid, state, name, user_uid) =
                    snapshot;

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

pub fn matches_search(proc: &ProcessInfo, query: &str) -> bool {
    let needle = query.trim();
    if needle.is_empty() {
        return true;
    }

    let matcher = SkimMatcherV2::default();
    let haystack = if proc.cmdline.is_empty() {
        proc.name.clone()
    } else {
        format!("{} {}", proc.name, proc.cmdline.join(" "))
    };

    matcher.fuzzy_match(&haystack, needle).is_some()
}
