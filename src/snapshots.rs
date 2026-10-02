//! Workspace snapshots: every tmux session, its windows and their layout, each pane's folder,
//! and the Claude conversation running in it. Saved every few minutes, so a restart (or a
//! tmux server that died) can rebuild the day's work.
//!
//! The rules: resume a conversation by its session id, never its name; build the panes, then
//! replay the layout, then `cd` and resume, because the layout reassigns panes by position;
//! never touch a session that is running; show the plan before restoring.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::transcript::transcript_path;
use crate::live::LiveSession;
use crate::tmux::{self, TmuxSession};

/// How many snapshots to keep (a new one is only written when something changed).
const KEEP: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Unix seconds.
    pub saved_at: u64,
    pub sessions: Vec<WsSession>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WsSession {
    pub name: String,
    pub windows: Vec<WsWindow>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WsWindow {
    pub index: String,
    pub name: String,
    pub layout: String,
    pub active: bool,
    pub panes: Vec<WsPane>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WsPane {
    pub index: String,
    pub cwd: String,
    pub claude: Option<WsClaude>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WsClaude {
    pub session_id: String,
    pub name: String,
    /// Flags worth replaying, with their values.
    pub flags: Vec<String>,
    /// The transcript was gone when saved, so resuming will fail.
    pub transcript_missing: bool,
}

const SKIP_PERMISSIONS: &str = "--dangerously-skip-permissions";

impl WsClaude {
    /// Resumes the conversation by its id, run the way it was: the same model, permission mode
    /// and folders. Settings in Claude Code's own files apply as they always do.
    pub fn resume_command(&self) -> String {
        let mut parts = vec!["claude".to_string()];
        parts.extend(self.flags.iter().cloned());
        parts.push("--resume".to_string());
        parts.push(self.session_id.clone());
        parts.join(" ")
    }
}

/// The flags of a running `claude` command line worth replaying. A prompt is never replayed
/// (it would run again), and neither is anything that picks or forks a session: the id is
/// taken from Claude Code's own records instead.
pub fn keep_flags(args: &str) -> Vec<String> {
    const WITH_VALUE: [&str; 3] = ["--model", "--permission-mode", "--add-dir"];
    let words: Vec<&str> = args.split_whitespace().collect();
    let mut kept = Vec::new();
    let mut i = 1;
    while i < words.len() {
        let word = words[i];
        if WITH_VALUE.contains(&word) && i + 1 < words.len() {
            kept.push(word.to_string());
            kept.push(words[i + 1].to_string());
            i += 2;
            continue;
        }
        if word == SKIP_PERMISSIONS || WITH_VALUE.iter().any(|flag| word.starts_with(&format!("{flag}="))) {
            kept.push(word.to_string());
        }
        i += 1;
    }
    kept
}

/// Today's tmux sessions (without the app's own views) and the Claude sessions in their panes.
/// `views` maps a session to the window the app shows for it: that's the window it reopens on.
pub fn capture(
    sessions: &[TmuxSession],
    views: &HashMap<String, String>,
    claude: &HashMap<String, LiveSession>,
    saved_at: u64,
) -> Workspace {
    let sessions = sessions
        .iter()
        .filter(|session| tmux::viewed_session(&session.name).is_none())
        .map(|session| WsSession {
            name: session.name.clone(),
            windows: session
                .windows
                .iter()
                .map(|window| WsWindow {
                    index: window.index.clone(),
                    name: window.name.clone(),
                    layout: window.layout.clone(),
                    active: views.get(&session.name).map_or(window.active, |id| *id == window.id),
                    panes: window
                        .panes
                        .iter()
                        .map(|pane| {
                            let live = claude.get(&pane.id);
                            WsPane {
                                index: pane.index.clone(),
                                // Claude resumes from the folder it started in.
                                cwd: live
                                    .map(|live| live.cwd.to_string_lossy().to_string())
                                    .filter(|cwd| !cwd.is_empty())
                                    .unwrap_or_else(|| pane.cwd.clone()),
                                claude: live.map(|live| WsClaude {
                                    session_id: live.session_id.clone(),
                                    name: live.name.clone(),
                                    flags: live.args.as_deref().map(keep_flags).unwrap_or_default(),
                                    transcript_missing: transcript_path(&live.cwd, &live.session_id).is_none(),
                                }),
                            }
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Workspace { saved_at, sessions }
}

pub fn dir() -> PathBuf {
    crate::sys::data_dir().join("workspaces")
}

/// Snapshot files, newest first.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(u64, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter_map(|path| {
            let stamp = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
            (path.extension()? == "json").then_some((stamp, path))
        })
        .collect();
    files.sort_by_key(|(stamp, _)| std::cmp::Reverse(*stamp));
    files.into_iter().map(|(_, path)| path).collect()
}

pub fn load(path: &Path) -> Option<Workspace> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Writes a snapshot unless nothing changed since the newest one, and keeps the newest
/// [`KEEP`]. A single slot would let one capture of a broken moment erase the good history.
pub fn save(dir: &Path, workspace: &Workspace) -> std::io::Result<Option<PathBuf>> {
    if workspace.sessions.is_empty() {
        return Ok(None);
    }
    let files = list(dir);
    if let Some(newest) = files.first().and_then(|path| load(path))
        && newest.sessions == workspace.sessions
    {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", workspace.saved_at));
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(workspace)?)?;
    std::fs::rename(&temp, &path)?;
    for old in files.iter().skip(KEEP - 1) {
        let _ = std::fs::remove_file(old);
    }
    Ok(Some(path))
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanSession {
    pub name: String,
    pub windows: usize,
    pub claudes: usize,
    pub missing_transcripts: usize,
}

/// What a restore would do: build the sessions that aren't running, leave the rest alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub create: Vec<PlanSession>,
    pub running: Vec<String>,
}

pub fn plan(workspace: &Workspace, running: &[String]) -> Plan {
    let mut plan = Plan::default();
    for session in &workspace.sessions {
        if running.contains(&session.name) {
            plan.running.push(session.name.clone());
            continue;
        }
        let claudes: Vec<&WsClaude> = session
            .windows
            .iter()
            .flat_map(|window| &window.panes)
            .filter_map(|pane| pane.claude.as_ref())
            .collect();
        plan.create.push(PlanSession {
            name: session.name.clone(),
            windows: session.windows.len(),
            claudes: claudes.len(),
            missing_transcripts: claudes.iter().filter(|claude| claude.transcript_missing).count(),
        });
    }
    plan
}

/// The newest snapshot, when it was saved before `boot`: the workspace as it was when the Mac
/// went down. One saved since means this boot's workspace is already being kept.
pub fn from_before(dir: &Path, boot: u64) -> Option<Workspace> {
    let newest = load(list(dir).first()?)?;
    (newest.saved_at < boot).then_some(newest)
}

/// Single quotes for a path typed into a shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The tmux commands that rebuild one session, in order. Panes are addressed by position from
/// `pane_base` (tmux's pane-base-index now), so a changed setting can't send a pane's command
/// to its neighbour.
pub fn restore_commands(session: &WsSession, first_window_index: &str, pane_base: usize) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let Some(first) = session.windows.first() else {
        return commands;
    };
    let name = &session.name;
    let first_cwd = first.panes.first().map(|pane| pane.cwd.clone()).unwrap_or_default();
    let words = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
    commands.push(words(&[
        "new-session",
        "-d",
        "-s",
        name,
        "-n",
        &first.name,
        "-c",
        &first_cwd,
    ]));
    if first.index != first_window_index {
        commands.push(words(&[
            "move-window",
            "-d",
            "-s",
            &format!("={name}:{first_window_index}"),
            "-t",
            &format!("={name}:{}", first.index),
        ]));
    }
    for (position, window) in session.windows.iter().enumerate() {
        let target = format!("={name}:{}", window.index);
        let cwd = window.panes.first().map(|pane| pane.cwd.clone()).unwrap_or_default();
        if position > 0 {
            commands.push(words(&[
                "new-window",
                "-d",
                "-t",
                &target,
                "-n",
                &window.name,
                "-c",
                &cwd,
            ]));
        }
        // Each split goes after the last pane, so panes keep their order (and their folders);
        // re-tiling between splits leaves room for the next one.
        let panes = window.panes.len();
        for (position, pane) in window.panes.iter().enumerate().skip(1) {
            let last = format!("{target}.{}", pane_base + position - 1);
            commands.push(words(&["split-window", "-d", "-t", &last, "-c", &pane.cwd]));
            if position + 1 < panes {
                commands.push(words(&["select-layout", "-t", &target, "tiled"]));
            }
        }
        if !window.layout.is_empty() {
            commands.push(words(&["select-layout", "-t", &target, &window.layout]));
        }
        for (position, pane) in window.panes.iter().enumerate() {
            if let Some(claude) = &pane.claude {
                let line = format!("cd {} && {}", shell_quote(&pane.cwd), claude.resume_command());
                let pane = format!("{target}.{}", pane_base + position);
                commands.push(words(&["send-keys", "-t", &pane, &line, "Enter"]));
            }
        }
    }
    if let Some(active) = session.windows.iter().find(|window| window.active) {
        commands.push(words(&["select-window", "-t", &format!("={name}:{}", active.index)]));
    }
    commands
}

/// Rebuilds every session in the snapshot that isn't running; running ones are never touched.
/// One line per session, for the person to read.
pub fn restore(workspace: &Workspace, running: &[String]) -> Vec<String> {
    let mut report = Vec::new();
    for session in &workspace.sessions {
        if running.contains(&session.name) {
            continue;
        }
        let commands = restore_commands(session, "", 0);
        let Some(create) = commands.first() else {
            continue;
        };
        let create: Vec<&str> = create.iter().map(String::as_str).collect();
        if !tmux::run(&create) {
            report.push(format!("Couldn't create {}", session.name));
            continue;
        }
        // The first window lands on the server's base index; move it if the snapshot differs.
        let base = tmux::display(&format!("={}:", session.name), "#{window_index}").unwrap_or_default();
        let pane_base = tmux::display(&format!("={}:", session.name), "#{pane-base-index}")
            .and_then(|base| base.parse().ok())
            .unwrap_or(0);
        let mut failed = 0;
        for command in restore_commands(session, &base, pane_base).iter().skip(1) {
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            if !tmux::run(&args) {
                failed += 1;
            }
        }
        let claudes = session
            .windows
            .iter()
            .flat_map(|window| &window.panes)
            .filter(|pane| pane.claude.is_some())
            .count();
        let windows = session.windows.len();
        let mut line = format!("{}: {windows} window{}", session.name, if windows == 1 { "" } else { "s" });
        if claudes > 0 {
            line.push_str(&format!(
                ", {claudes} Claude conversation{} resumed",
                if claudes == 1 { "" } else { "s" }
            ));
        }
        if failed > 0 {
            line.push_str(&format!(" ({failed} step{} failed)", if failed == 1 { "" } else { "s" }));
        }
        report.push(line);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::TmuxWindow;

    fn claude(id: &str) -> WsClaude {
        WsClaude {
            session_id: id.into(),
            name: "Planner".into(),
            flags: vec![],
            transcript_missing: false,
        }
    }

    fn workspace() -> Workspace {
        Workspace {
            saved_at: 100,
            sessions: vec![
                WsSession {
                    name: "web".into(),
                    windows: vec![
                        WsWindow {
                            index: "1".into(),
                            name: "frontend".into(),
                            layout: "ab12,200x50,0,0{100x50,0,0,1,99x50,101,0,2}".into(),
                            active: false,
                            panes: vec![
                                WsPane {
                                    index: "1".into(),
                                    cwd: "/code/web".into(),
                                    claude: Some(claude("aaa")),
                                },
                                WsPane {
                                    index: "2".into(),
                                    cwd: "/code/it's".into(),
                                    claude: None,
                                },
                            ],
                        },
                        WsWindow {
                            index: "2".into(),
                            name: "planner".into(),
                            layout: String::new(),
                            active: true,
                            panes: vec![WsPane {
                                index: "1".into(),
                                cwd: "/code/plan".into(),
                                claude: Some(claude("bbb")),
                            }],
                        },
                    ],
                },
                WsSession {
                    name: "ops".into(),
                    windows: vec![],
                },
            ],
        }
    }

    #[test]
    fn flags_keep_values_and_drop_prompts_and_forks() {
        let args = "claude --dangerously-skip-permissions --fork-session --resume x --model opus Do the thing";
        assert_eq!(keep_flags(args), ["--dangerously-skip-permissions", "--model", "opus"]);
        assert_eq!(keep_flags("claude --permission-mode=auto"), ["--permission-mode=auto"]);
        assert!(keep_flags("claude").is_empty());
    }

    #[test]
    fn resume_by_id_the_way_it_ran() {
        let mut c = claude("abc-123");
        assert_eq!(c.resume_command(), "claude --resume abc-123");
        c.flags = vec!["--dangerously-skip-permissions".into(), "--model".into(), "opus".into()];
        assert_eq!(
            c.resume_command(),
            "claude --dangerously-skip-permissions --model opus --resume abc-123"
        );
    }

    #[test]
    fn plan_leaves_running_sessions_alone() {
        let plan = plan(&workspace(), &["ops".to_string()]);
        assert_eq!(plan.running, ["ops"]);
        assert_eq!(
            plan.create,
            [PlanSession {
                name: "web".into(),
                windows: 2,
                claudes: 2,
                missing_transcripts: 0
            }]
        );
    }

    #[test]
    fn panes_split_in_order_with_room_for_each() {
        let pane = |cwd: &str| WsPane {
            index: String::new(),
            cwd: cwd.into(),
            claude: None,
        };
        let session = WsSession {
            name: "Lab".into(),
            windows: vec![WsWindow {
                index: "1".into(),
                name: "grid".into(),
                layout: String::new(),
                active: true,
                panes: vec![pane("/a"), pane("/b"), pane("/c")],
            }],
        };
        let joined: Vec<String> = restore_commands(&session, "1", 1).iter().map(|c| c.join(" ")).collect();
        assert_eq!(
            joined[1..4],
            [
                "split-window -d -t =Lab:1.1 -c /b",
                "select-layout -t =Lab:1 tiled",
                "split-window -d -t =Lab:1.2 -c /c",
            ]
        );
        assert!(!joined.iter().any(|command| command.starts_with("send-keys")));
    }

    #[test]
    fn a_viewed_session_reopens_on_the_window_its_view_shows() {
        let window = |id: &str, active: bool| TmuxWindow {
            id: id.into(),
            index: id.trim_start_matches('@').into(),
            name: id.into(),
            active,
            layout: String::new(),
            panes: vec![],
        };
        let sessions = vec![TmuxSession {
            name: "web".into(),
            windows: vec![window("@1", true), window("@2", false)],
        }];
        let active = |workspace: &Workspace| -> Vec<bool> {
            workspace.sessions[0].windows.iter().map(|window| window.active).collect()
        };
        let viewed = HashMap::from([("web".to_string(), "@2".to_string())]);
        assert_eq!(active(&capture(&sessions, &viewed, &HashMap::new(), 1)), [false, true]);
        assert_eq!(active(&capture(&sessions, &HashMap::new(), &HashMap::new(), 1)), [true, false]);
    }

    #[test]
    fn commands_build_panes_then_layout_then_resume() {
        let ws = workspace();
        let commands = restore_commands(&ws.sessions[0], "1", 1);
        let joined: Vec<String> = commands.iter().map(|c| c.join(" ")).collect();
        assert_eq!(joined[0], "new-session -d -s web -n frontend -c /code/web");
        assert_eq!(joined[1], "split-window -d -t =web:1.1 -c /code/it's");
        assert!(joined[2].starts_with("select-layout -t =web:1 ab12,"));
        // Only the Claude pane gets a command; the plain pane opens in its folder.
        assert_eq!(
            joined[3],
            "send-keys -t =web:1.1 cd '/code/web' && claude --resume aaa Enter"
        );
        assert_eq!(joined[4], "new-window -d -t =web:2 -n planner -c /code/plan");
        assert_eq!(joined.last().unwrap(), "select-window -t =web:2");
        let moved = restore_commands(&ws.sessions[0], "0", 0);
        assert_eq!(moved[1].join(" "), "move-window -d -s =web:0 -t =web:1");
        // With panes counted from 0, the first pane is .0 whatever the snapshot numbered it.
        assert_eq!(moved[2].join(" "), "split-window -d -t =web:1.0 -c /code/it's");
        assert!(moved[4].join(" ").starts_with("send-keys -t =web:1.0 cd '/code/web'"));
    }

    #[test]
    fn saves_only_changes_and_finds_the_workspace_from_before_boot() {
        let dir = std::env::temp_dir().join(format!("signalbox-snapshots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = workspace();
        assert!(save(&dir, &first).unwrap().is_some());
        let mut same = first.clone();
        same.saved_at = 200;
        assert!(save(&dir, &same).unwrap().is_none());
        assert_eq!(list(&dir).len(), 1);

        // Saved at 100: a Mac started at 150 rebuilds it, minus what's running; one started at
        // 50 has saved since, so there's nothing to rebuild.
        let rebuild = from_before(&dir, 150).unwrap();
        assert_eq!(rebuild.saved_at, 100);
        assert_eq!(plan(&rebuild, &["ops".into()]).create.len(), 1);
        assert!(plan(&rebuild, &["web".into(), "ops".into()]).create.is_empty());
        assert!(from_before(&dir, 50).is_none());

        let empty = Workspace {
            saved_at: 300,
            ..Default::default()
        };
        assert!(save(&dir, &empty).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod machine {
    use super::*;

    /// Run by hand: `cargo test snapshots::machine::save_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn save_this_machine() {
        let claude = crate::live::scan()
            .into_iter()
            .filter_map(|session| Some((session.pane.clone()?, session)))
            .collect();
        let saved_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let workspace = capture(&tmux::sessions().expect("tmux"), &HashMap::new(), &claude, saved_at);
        for session in &workspace.sessions {
            let claudes: Vec<String> = session
                .windows
                .iter()
                .flat_map(|w| &w.panes)
                .filter_map(|p| p.claude.as_ref())
                .map(|c| format!("{} {:?}", c.name, c.flags))
                .collect();
            println!(
                "{}: {} windows; claude: {:?}",
                session.name,
                session.windows.len(),
                claudes
            );
        }
        println!("saved: {:?}", save(&dir(), &workspace).unwrap());
    }

    /// Run by hand: `SIGNALBOX_RESTORE=<session> cargo test snapshots::machine::restore_one -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn restore_one() {
        let name = std::env::var("SIGNALBOX_RESTORE").expect("SIGNALBOX_RESTORE=<session>");
        let newest = list(&dir()).first().and_then(|path| load(path)).expect("a snapshot");
        let only = Workspace {
            sessions: newest.sessions.into_iter().filter(|s| s.name == name).collect(),
            ..newest
        };
        let running: Vec<String> = tmux::sessions().expect("tmux").into_iter().map(|s| s.name).collect();
        println!("{:?}", plan(&only, &running));
        println!("{:?}", restore(&only, &running));
    }
}
