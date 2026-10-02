//! Claude Code sessions running in terminals, from the files Claude Code keeps in
//! `~/.claude/sessions/<pid>.json` (name, status, what it waits for, its tmux pane).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::status::{self, SessionStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeState {
    /// Waiting on the person: a permission prompt, a question, a dialog.
    NeedsYou(Option<String>),
    Working,
    /// Running a shell command the person typed, or suspended to the shell.
    Shell,
    Idle,
}

impl ClaudeState {
    /// Higher wins when several sessions share a window or session.
    fn rank(&self) -> u8 {
        match self {
            ClaudeState::NeedsYou(_) => 3,
            ClaudeState::Working => 2,
            ClaudeState::Shell => 1,
            ClaudeState::Idle => 0,
        }
    }

    pub fn most_urgent(a: Option<ClaudeState>, b: ClaudeState) -> ClaudeState {
        match a {
            Some(a) if a.rank() >= b.rank() => a,
            _ => b,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    pub pid: u32,
    pub session_id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub state: ClaudeState,
    /// tmux pane id, e.g. `%103`.
    pub pane: Option<String>,
    /// What its status line last reported.
    pub status: Option<SessionStatus>,
    /// The command line it runs with.
    pub args: Option<String>,
    /// When its process started (unix milliseconds).
    pub started_at: Option<u64>,
    proc_start: Option<String>,
}

pub fn sessions_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".claude").join("sessions")
}

/// Interactive terminal sessions whose process is still the one that wrote the file.
pub fn scan() -> Vec<LiveSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else {
        return Vec::new();
    };
    let sessions: Vec<LiveSession> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
        .filter_map(|value| parse(&value))
        .collect();
    let running = processes(sessions.iter().map(|session| session.pid));
    sessions
        .into_iter()
        .filter_map(|mut session| {
            let (started, args) = running.get(&session.pid)?;
            if let Some(recorded) = &session.proc_start
                && !same_words(started, recorded)
            {
                return None;
            }
            session.status = status::read(&session.session_id);
            session.args = Some(args.clone());
            Some(session)
        })
        .collect()
}

/// One session file, if it describes an interactive terminal session.
pub fn parse(value: &Value) -> Option<LiveSession> {
    // Claude Code keeps a spare process warm for the next session; it isn't a conversation.
    if value["entrypoint"].as_str() != Some("cli") || value["spare"] == true {
        return None;
    }
    let pid = value["pid"].as_u64()? as u32;
    let cwd = PathBuf::from(value["cwd"].as_str().unwrap_or_default());
    let name = value["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| cwd.file_name().map(|name| name.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Claude".to_string());
    let waiting_for = value["waitingFor"].as_str().map(str::to_string);
    let state = match (value["status"].as_str(), waiting_for) {
        (_, Some(what)) => ClaudeState::NeedsYou(Some(what)),
        (Some("waiting"), None) => ClaudeState::NeedsYou(None),
        (Some("busy"), None) => ClaudeState::Working,
        (Some("shell"), None) => ClaudeState::Shell,
        _ => ClaudeState::Idle,
    };
    let pane = value["tmux"]
        .as_str()
        .and_then(|tmux| tmux.rsplit_once('.'))
        .map(|(_, pane)| pane.to_string())
        .filter(|pane| pane.starts_with('%'));
    Some(LiveSession {
        pid,
        session_id: value["sessionId"].as_str().unwrap_or_default().to_string(),
        name,
        cwd,
        state,
        pane,
        status: None,
        args: None,
        started_at: value["startedAt"].as_u64(),
        proc_start: value["procStart"].as_str().map(str::to_string),
    })
}

/// Start time (UTC, in `ps` format) and command line of the running processes among `pids`.
fn processes(pids: impl Iterator<Item = u32>) -> HashMap<u32, (String, String)> {
    let list: Vec<String> = pids.map(|pid| pid.to_string()).collect();
    if list.is_empty() {
        return HashMap::new();
    }
    // Claude Code records procStart in UTC.
    let Ok(output) = crate::sys::command("/bin/ps")
        .args(["-o", "pid=,lstart=,args=", "-p", &list.join(",")])
        .env("TZ", "UTC")
        .output()
    else {
        return HashMap::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_process_line)
        .collect()
}

/// "  160 Thu Oct  1 08:31:02 2026     claude --resume x": the start time is always 24 characters.
fn parse_process_line(line: &str) -> Option<(u32, (String, String))> {
    let line = line.trim_start();
    let (pid, rest) = line.split_once(' ')?;
    let rest = rest.trim_start();
    let started = rest.get(..24)?;
    Some((pid.parse().ok()?, (started.to_string(), rest[24..].trim().to_string())))
}

/// `ps` pads single-digit days with an extra space; compare word by word.
fn same_words(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_terminal_session() {
        let session = parse(&json!({
            "pid": 21074, "entrypoint": "cli", "status": "busy", "name": "Planner",
            "cwd": "/Users/me/code/app", "sessionId": "b9f9", "tmux": "web:@75.%103",
            "procStart": "Wed Sep 30 06:18:03 2026"
        }))
        .unwrap();
        assert_eq!(session.name, "Planner");
        assert_eq!(session.state, ClaudeState::Working);
        assert_eq!(session.pane.as_deref(), Some("%103"));
    }

    #[test]
    fn waiting_and_unnamed_sessions() {
        let session = parse(&json!({
            "pid": 1, "entrypoint": "cli", "status": "busy", "waitingFor": "input needed", "cwd": "/x/site"
        }))
        .unwrap();
        assert_eq!(session.state, ClaudeState::NeedsYou(Some("input needed".into())));
        assert_eq!(session.name, "site");
        assert_eq!(session.pane, None);
    }

    #[test]
    fn headless_sessions_are_skipped() {
        assert!(parse(&json!({"pid": 2, "entrypoint": "sdk-cli", "status": "idle"})).is_none());
    }

    #[test]
    fn urgency_order() {
        let idle = ClaudeState::most_urgent(None, ClaudeState::Idle);
        let working = ClaudeState::most_urgent(Some(idle), ClaudeState::Working);
        assert_eq!(working, ClaudeState::Working);
        let still = ClaudeState::most_urgent(Some(ClaudeState::NeedsYou(None)), ClaudeState::Working);
        assert_eq!(still, ClaudeState::NeedsYou(None));
    }

    #[test]
    fn process_lines() {
        let (pid, (started, args)) =
            parse_process_line("  160 Thu Oct  1 08:31:02 2026     claude --dangerously-skip-permissions").unwrap();
        assert_eq!(pid, 160);
        assert_eq!(started, "Thu Oct  1 08:31:02 2026");
        assert_eq!(args, "claude --dangerously-skip-permissions");
    }

    #[test]
    fn spare_processes_are_not_sessions() {
        assert!(parse(&json!({"pid": 3, "entrypoint": "cli", "spare": true, "status": "idle"})).is_none());
    }

    #[test]
    fn start_times_compare_by_words() {
        assert!(same_words("Thu Oct  1 08:44:54 2026", "Thu Oct 1 08:44:54 2026"));
        assert!(!same_words("Thu Oct  1 08:44:54 2026", "Thu Oct 2 08:44:54 2026"));
    }
}

#[cfg(test)]
mod machine {
    /// Run by hand: `cargo test live::machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn scan_this_machine() {
        let sessions = super::scan();
        for session in &sessions {
            println!(
                "{} {:?} {:?} {}",
                session.pid, session.pane, session.state, session.name
            );
        }
        println!("{} sessions", sessions.len());
    }
}
