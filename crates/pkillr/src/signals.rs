use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use nix::errno::Errno;
use nix::sys::signal::{Signal as NixSignal, kill};
use nix::unistd::{Pid as NixPid, Uid, User};

use crate::process::{ProcessInfo, ProcessManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Sighup,
    Sigint,
    Sigquit,
    Sigill,
    Sigtrap,
    Sigabrt,
    Sigbus,
    Sigfpe,
    Sigkill,
    Sigusr1,
    Sigsegv,
    Sigusr2,
    Sigpipe,
    Sigalrm,
    Sigterm,
    Sigstkflt,
    Sigchld,
    Sigcont,
    Sigstop,
    Sigtstp,
    Sigttin,
    Sigttou,
    Sigurg,
    Sigxcpu,
    Sigxfsz,
    Sigvtalrm,
    Sigprof,
    Sigwinch,
    Sigio,
    Sigpwr,
    Sigsys,
}

const ALL_SIGNALS: [Signal; 31] = [
    Signal::Sighup,
    Signal::Sigint,
    Signal::Sigquit,
    Signal::Sigill,
    Signal::Sigtrap,
    Signal::Sigabrt,
    Signal::Sigbus,
    Signal::Sigfpe,
    Signal::Sigkill,
    Signal::Sigusr1,
    Signal::Sigsegv,
    Signal::Sigusr2,
    Signal::Sigpipe,
    Signal::Sigalrm,
    Signal::Sigterm,
    Signal::Sigstkflt,
    Signal::Sigchld,
    Signal::Sigcont,
    Signal::Sigstop,
    Signal::Sigtstp,
    Signal::Sigttin,
    Signal::Sigttou,
    Signal::Sigurg,
    Signal::Sigxcpu,
    Signal::Sigxfsz,
    Signal::Sigvtalrm,
    Signal::Sigprof,
    Signal::Sigwinch,
    Signal::Sigio,
    Signal::Sigpwr,
    Signal::Sigsys,
];

impl Signal {
    pub const fn all() -> &'static [Signal] {
        &ALL_SIGNALS
    }

    pub fn number(self) -> i32 {
        match self {
            Signal::Sighup => 1,
            Signal::Sigint => 2,
            Signal::Sigquit => 3,
            Signal::Sigill => 4,
            Signal::Sigtrap => 5,
            Signal::Sigabrt => 6,
            Signal::Sigbus => 7,
            Signal::Sigfpe => 8,
            Signal::Sigkill => 9,
            Signal::Sigusr1 => 10,
            Signal::Sigsegv => 11,
            Signal::Sigusr2 => 12,
            Signal::Sigpipe => 13,
            Signal::Sigalrm => 14,
            Signal::Sigterm => 15,
            Signal::Sigstkflt => 16,
            Signal::Sigchld => 17,
            Signal::Sigcont => 18,
            Signal::Sigstop => 19,
            Signal::Sigtstp => 20,
            Signal::Sigttin => 21,
            Signal::Sigttou => 22,
            Signal::Sigurg => 23,
            Signal::Sigxcpu => 24,
            Signal::Sigxfsz => 25,
            Signal::Sigvtalrm => 26,
            Signal::Sigprof => 27,
            Signal::Sigwinch => 28,
            Signal::Sigio => 29,
            Signal::Sigpwr => 30,
            Signal::Sigsys => 31,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Signal::Sighup => "SIGHUP",
            Signal::Sigint => "SIGINT",
            Signal::Sigquit => "SIGQUIT",
            Signal::Sigill => "SIGILL",
            Signal::Sigtrap => "SIGTRAP",
            Signal::Sigabrt => "SIGABRT",
            Signal::Sigbus => "SIGBUS",
            Signal::Sigfpe => "SIGFPE",
            Signal::Sigkill => "SIGKILL",
            Signal::Sigusr1 => "SIGUSR1",
            Signal::Sigsegv => "SIGSEGV",
            Signal::Sigusr2 => "SIGUSR2",
            Signal::Sigpipe => "SIGPIPE",
            Signal::Sigalrm => "SIGALRM",
            Signal::Sigterm => "SIGTERM",
            Signal::Sigstkflt => "SIGSTKFLT",
            Signal::Sigchld => "SIGCHLD",
            Signal::Sigcont => "SIGCONT",
            Signal::Sigstop => "SIGSTOP",
            Signal::Sigtstp => "SIGTSTP",
            Signal::Sigttin => "SIGTTIN",
            Signal::Sigttou => "SIGTTOU",
            Signal::Sigurg => "SIGURG",
            Signal::Sigxcpu => "SIGXCPU",
            Signal::Sigxfsz => "SIGXFSZ",
            Signal::Sigvtalrm => "SIGVTALRM",
            Signal::Sigprof => "SIGPROF",
            Signal::Sigwinch => "SIGWINCH",
            Signal::Sigio => "SIGIO",
            Signal::Sigpwr => "SIGPWR",
            Signal::Sigsys => "SIGSYS",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Signal::Sighup => "reload config",
            Signal::Sigint => "interrupt",
            Signal::Sigquit => "quit & core dump",
            Signal::Sigill => "illegal instruction",
            Signal::Sigtrap => "trace trap",
            Signal::Sigabrt => "abort",
            Signal::Sigbus => "bus error",
            Signal::Sigfpe => "floating point exception",
            Signal::Sigkill => "force kill",
            Signal::Sigusr1 => "user signal 1",
            Signal::Sigsegv => "segmentation fault",
            Signal::Sigusr2 => "user signal 2",
            Signal::Sigpipe => "broken pipe",
            Signal::Sigalrm => "alarm",
            Signal::Sigterm => "graceful shutdown",
            Signal::Sigstkflt => "stack fault",
            Signal::Sigchld => "child status change",
            Signal::Sigcont => "resume",
            Signal::Sigstop => "stop process",
            Signal::Sigtstp => "terminal stop",
            Signal::Sigttin => "background read",
            Signal::Sigttou => "background write",
            Signal::Sigurg => "urgent condition",
            Signal::Sigxcpu => "cpu time limit exceeded",
            Signal::Sigxfsz => "file size limit exceeded",
            Signal::Sigvtalrm => "virtual alarm",
            Signal::Sigprof => "profiling timer",
            Signal::Sigwinch => "window resize",
            Signal::Sigio => "asynchronous i/o",
            Signal::Sigpwr => "power failure",
            Signal::Sigsys => "bad system call",
        }
    }

    fn to_nix(self) -> Result<NixSignal, String> {
        NixSignal::try_from(self.number())
            .map_err(|_| format!("signal {} not available on this platform", self.name()))
    }
}

impl Default for Signal {
    fn default() -> Self {
        Signal::Sigterm
    }
}

#[derive(Debug, Clone)]
pub struct SignalEvent {
    pub timestamp: DateTime<Utc>,
    pub pid: u32,
    pub process_name: String,
    pub signal: Signal,
    pub result: Result<(), String>,
}

pub struct SignalSender {
    manager: ProcessManager,
    history: VecDeque<SignalEvent>,
}

impl SignalSender {
    pub fn new() -> Self {
        Self {
            manager: ProcessManager::new(),
            history: VecDeque::with_capacity(10),
        }
    }

    pub fn history(&self) -> impl Iterator<Item = &SignalEvent> {
        self.history.iter().rev()
    }

    pub fn send_signal(&mut self, pid: u32, signal: Signal) -> Result<(), String> {
        match send_signal_with_manager(&mut self.manager, pid, signal) {
            Ok(info) => {
                self.push_event(SignalEvent {
                    timestamp: Utc::now(),
                    pid,
                    process_name: info.name.clone(),
                    signal,
                    result: Ok(()),
                });
                Ok(())
            }
            Err(err) => {
                let name = self
                    .lookup_process(pid)
                    .map(|proc| proc.name)
                    .unwrap_or_else(|| "unknown".to_string());
                self.push_event(SignalEvent {
                    timestamp: Utc::now(),
                    pid,
                    process_name: name,
                    signal,
                    result: Err(err.clone()),
                });
                Err(err)
            }
        }
    }

    pub fn kill_process_tree(&mut self, root_pid: u32, signal: Signal) -> Result<Vec<u32>, String> {
        let mut events = Vec::new();
        let outcome =
            kill_process_tree_with_manager(&mut self.manager, root_pid, signal, &mut events);
        for event in events {
            self.push_event(event);
        }
        outcome
    }

    fn push_event(&mut self, event: SignalEvent) {
        if self.history.len() == 10 {
            self.history.pop_front();
        }
        self.history.push_back(event);
    }

    fn lookup_process(&mut self, pid: u32) -> Option<ProcessInfo> {
        self.manager
            .get_processes(true)
            .into_iter()
            .find(|proc| proc.pid == pid)
    }
}

pub fn send_signal(pid: u32, signal: Signal) -> Result<(), String> {
    let mut manager = ProcessManager::new();
    send_signal_with_manager(&mut manager, pid, signal).map(|_| ())
}

pub fn kill_process_tree(root_pid: u32, signal: Signal) -> Result<Vec<u32>, String> {
    let mut manager = ProcessManager::new();
    let mut events = Vec::new();
    kill_process_tree_with_manager(&mut manager, root_pid, signal, &mut events)
}

fn send_signal_with_manager(
    manager: &mut ProcessManager,
    pid: u32,
    signal: Signal,
) -> Result<ProcessInfo, String> {
    let info = lookup(manager, pid)?;
    validate_target(&info)?;
    ensure_permissions(&info)?;
    send_to_pid(pid, signal)?;
    Ok(info)
}

fn kill_process_tree_with_manager(
    manager: &mut ProcessManager,
    root_pid: u32,
    signal: Signal,
    events: &mut Vec<SignalEvent>,
) -> Result<Vec<u32>, String> {
    if root_pid == 1 {
        return Err("refusing to signal pid 1".to_string());
    }
    if root_pid == std::process::id() {
        return Err("refusing to signal pkillr".to_string());
    }

    let tree = collect_tree(manager, root_pid);
    let mut killed = Vec::new();

    for pid in tree {
        let info = match lookup(manager, pid) {
            Ok(info) => info,
            Err(err) => {
                events.push(SignalEvent {
                    timestamp: Utc::now(),
                    pid,
                    process_name: "unknown".to_string(),
                    signal,
                    result: Err(err.clone()),
                });
                return Err(format!("failed after killing {:?}: {}", killed, err));
            }
        };

        let result = validate_target(&info)
            .and_then(|_| ensure_permissions(&info))
            .and_then(|_| send_to_pid(pid, signal));

        events.push(SignalEvent {
            timestamp: Utc::now(),
            pid,
            process_name: info.name.clone(),
            signal,
            result: result.clone(),
        });

        match result {
            Ok(()) => killed.push(pid),
            Err(err) => {
                return Err(format!("failed after killing {:?}: {}", killed, err));
            }
        }
    }

    Ok(killed)
}

fn lookup(manager: &mut ProcessManager, pid: u32) -> Result<ProcessInfo, String> {
    manager
        .get_processes(true)
        .into_iter()
        .find(|proc| proc.pid == pid)
        .ok_or_else(|| "process not found".to_string())
}

fn validate_target(info: &ProcessInfo) -> Result<(), String> {
    if info.pid == 1 {
        return Err("refusing to signal pid 1".to_string());
    }
    if info.pid == std::process::id() {
        return Err("refusing to signal pkillr".to_string());
    }
    Ok(())
}

fn ensure_permissions(info: &ProcessInfo) -> Result<(), String> {
    let current_uid = Uid::current();
    if current_uid.as_raw() == 0 {
        return Ok(());
    }

    let current_user = User::from_uid(current_uid)
        .ok()
        .flatten()
        .map(|user| user.name)
        .ok_or_else(|| "cannot determine current user".to_string())?;

    if info.user == "unknown" {
        return Err("permission denied (needs sudo)".to_string());
    }

    if info.user != current_user {
        return Err("permission denied (needs sudo)".to_string());
    }

    Ok(())
}

fn send_to_pid(pid: u32, signal: Signal) -> Result<(), String> {
    let nix_signal = signal.to_nix()?;
    match kill(NixPid::from_raw(pid as i32), nix_signal) {
        Ok(()) => Ok(()),
        Err(Errno::EPERM) => Err("permission denied (needs sudo)".to_string()),
        Err(Errno::ESRCH) => Err("process not found".to_string()),
        Err(err) => Err(format!("failed to send {}: {}", signal.name(), err)),
    }
}

fn collect_tree(manager: &mut ProcessManager, root_pid: u32) -> Vec<u32> {
    let processes = manager.get_processes(true);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

    for process in &processes {
        if let Some(parent) = process.parent_pid {
            children.entry(parent).or_default().push(process.pid);
        }
    }

    let mut order = Vec::new();
    let mut visited = HashSet::new();
    post_order(root_pid, &children, &mut visited, &mut order);
    order
}

fn post_order(
    pid: u32,
    children: &HashMap<u32, Vec<u32>>,
    visited: &mut HashSet<u32>,
    order: &mut Vec<u32>,
) {
    if !visited.insert(pid) {
        return;
    }

    if let Some(kids) = children.get(&pid) {
        for child in kids {
            post_order(*child, children, visited, order);
        }
    }

    order.push(pid);
}
