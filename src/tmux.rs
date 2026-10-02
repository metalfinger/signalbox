//! The tmux server's sessions, windows and panes, read with `tmux list-panes`.

use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxPane {
    /// Stable id, e.g. `%103`.
    pub id: String,
    pub index: String,
    /// The folder of the program in the pane.
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxWindow {
    /// Stable id, e.g. `@75`.
    pub id: String,
    pub index: String,
    pub name: String,
    pub active: bool,
    /// tmux's layout string; `select-layout` replays it exactly.
    pub layout: String,
    pub panes: Vec<TmuxPane>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub windows: Vec<TmuxWindow>,
}

const PANE_FORMAT: &str = "#{session_name}\t#{window_id}\t#{window_index}\t#{window_name}\t#{window_active}\t\
                           #{window_layout}\t#{pane_id}\t#{pane_index}\t#{pane_current_path}";

/// Every query asks for UTF-8 (`-u`). Otherwise tmux answers a client without a UTF-8 locale
/// (an app opened from Finder or Spotlight has none) with each tab and non-ASCII character
/// replaced by `_`, and nothing in the answer can be read.
fn tmux() -> Command {
    let mut command = crate::sys::command(crate::sys::find_program("tmux"));
    command.env("PATH", crate::sys::search_path()).env_remove("TMUX").arg("-u");
    command
}

/// Every session with its windows, in tmux's order: empty when no server is running, an error
/// when tmux couldn't be asked or answered with one.
pub fn sessions() -> Result<Vec<TmuxSession>, String> {
    let output = tmux()
        .args(["list-panes", "-a", "-F", PANE_FORMAT])
        .output()
        .map_err(|error| format!("Couldn't run tmux: {error}"))?;
    if output.status.success() {
        return read_panes(&String::from_utf8_lossy(&output.stdout));
    }
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if no_server(&error) {
        return Ok(Vec::new());
    }
    Err(if error.is_empty() {
        format!("tmux failed ({})", output.status)
    } else {
        error
    })
}

/// tmux's list of panes, read. An answer with lines but none readable is an error: shown as
/// no sessions, it would hide every running one.
fn read_panes(text: &str) -> Result<Vec<TmuxSession>, String> {
    let sessions = parse_panes(text);
    if sessions.is_empty() && text.lines().any(|line| !line.trim().is_empty()) {
        return Err("tmux answered in a form Signalbox can't read.".to_string());
    }
    Ok(sessions)
}

/// tmux's answer when no server is running: nothing to list, rather than a failure.
fn no_server(error: &str) -> bool {
    error.starts_with("no server running")
        || (error.starts_with("error connecting to") && error.ends_with("(No such file or directory)"))
}

/// One line per pane, in tmux's order, grouped into sessions and windows.
pub fn parse_panes(text: &str) -> Vec<TmuxSession> {
    let mut sessions: Vec<TmuxSession> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [session, id, index, name, active, layout, pane, pane_index, cwd] = fields[..] else {
            continue;
        };
        if sessions.last().is_none_or(|last| last.name != session) {
            sessions.push(TmuxSession {
                name: session.to_string(),
                windows: Vec::new(),
            });
        }
        let windows = &mut sessions.last_mut().expect("pushed above").windows;
        if windows.last().is_none_or(|last| last.id != id) {
            windows.push(TmuxWindow {
                id: id.to_string(),
                index: index.to_string(),
                name: name.to_string(),
                active: active == "1",
                layout: layout.to_string(),
                panes: Vec::new(),
            });
        }
        windows.last_mut().expect("pushed above").panes.push(TmuxPane {
            id: pane.to_string(),
            index: pane_index.to_string(),
            cwd: cwd.to_string(),
        });
    }
    sessions
}

/// A tab views a session through its own session in the same group: the same windows and
/// panes, but its own current window, so choosing a window here never moves a terminal that
/// is attached to the original, or another tab. tmux removes the view when the tab's client
/// detaches. The first view of a session is `<session>·signalbox`, the next `<session>·signalbox2`.
const VIEW_SUFFIX: &str = "·signalbox";

pub fn view_name(session: &str, number: usize) -> String {
    match number {
        0 | 1 => format!("{session}{VIEW_SUFFIX}"),
        n => format!("{session}{VIEW_SUFFIX}{n}"),
    }
}

/// The original session a view belongs to, if `name` is one of the app's views.
pub fn viewed_session(name: &str) -> Option<&str> {
    let (session, number) = name.rsplit_once(VIEW_SUFFIX)?;
    number.chars().all(|c| c.is_ascii_digit()).then_some(session)
}

/// Runs a tmux command; true when it succeeded.
pub fn run(args: &[&str]) -> bool {
    tmux().args(args).output().is_ok_and(|output| output.status.success())
}

/// Reads one value with `display-message`, e.g. `#{window_index}` of a target.
pub fn display(target: &str, format: &str) -> Option<String> {
    let output = tmux()
        .args(["display-message", "-p", "-t", target, format])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// One step of searching the history of the pane a tab shows, with tmux's own search (it
/// holds the scrollback, and highlights what it finds).
#[derive(Clone, Debug, PartialEq)]
pub enum Search {
    /// Search up from where the search began; each new text starts over from there.
    Find(String),
    /// The next match up (older), or down (newer).
    Older,
    Newer,
    /// Leave copy mode, back to the live pane.
    Done,
}

pub fn search_args(view: &str, step: &Search) -> Vec<String> {
    let target = format!("={view}:");
    let copy = |command: &str| ["send-keys", "-t", &target, "-X", command].map(str::to_string).to_vec();
    match step {
        // `=` is tmux's prefix for "search again from the start" in an incremental search.
        Search::Find(text) => {
            let mut args = ["copy-mode", "-t", &target, ";"].map(str::to_string).to_vec();
            args.extend(copy("search-backward-incremental"));
            args.push(format!("={text}"));
            args
        }
        Search::Older => copy("search-again"),
        Search::Newer => copy("search-reverse"),
        Search::Done => copy("cancel"),
    }
}

/// The command a tab runs to show `session` through its own `view` (see `view_name`), opening
/// on `window_id` when given.
pub fn attach_command(session: &str, view: &str, window_id: Option<&str>) -> (String, Vec<String>) {
    // `hyperlinks`: tmux passes on the links programs print (Claude Code's PR and file links are
    // hidden behind their text) only to terminals it knows can open them.
    let mut args: Vec<String> = [
        "-u",
        "-T",
        "hyperlinks",
        "new-session",
        "-A",
        "-s",
        view,
        "-t",
        &format!("={session}"),
        ";",
        "set-option",
        "-q",
        "destroy-unattached",
        "on",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    if let Some(window_id) = window_id {
        args.extend([
            ";".to_string(),
            "select-window".to_string(),
            "-t".to_string(),
            format!("={view}:{window_id}"),
        ]);
    }
    (crate::sys::find_program("tmux").to_string_lossy().to_string(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_asks_tmux_for_utf8() {
        assert_eq!(tmux().get_args().next(), Some(std::ffi::OsStr::new("-u")));
    }

    #[test]
    fn an_answer_that_cant_be_read_is_an_error_not_an_empty_list() {
        // What tmux sends a client without a UTF-8 locale: every tab replaced by `_`.
        let mangled = "web_@35_1_docs site_0_ab12,200x50,0,0_%53_1_/code/a\n";
        assert!(read_panes(mangled).is_err());
        assert!(read_panes("").is_ok_and(|sessions| sessions.is_empty()));
    }

    #[test]
    fn groups_panes_by_window_and_session() {
        let text = "web\t@35\t1\tdocs site\t0\tab12,200x50,0,0\t%53\t1\t/code/a\n\
                    web\t@35\t1\tdocs site\t0\tab12,200x50,0,0\t%99\t2\t/code/b\n\
                    web\t@75\t2\tplanner\t1\tcd34,200x50,0,0,7\t%103\t1\t/code/c\n\
                    api\t@4\t1\tzsh\t1\tef56,80x24,0,0,1\t%7\t1\t/x\nbad line\n";
        let sessions = parse_panes(text);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "web");
        assert_eq!(sessions[0].windows.len(), 2);
        let panes: Vec<&str> = sessions[0].windows[0].panes.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(panes, ["%53", "%99"]);
        assert_eq!(sessions[0].windows[0].panes[1].cwd, "/code/b");
        assert_eq!(sessions[0].windows[0].layout, "ab12,200x50,0,0");
        assert_eq!(sessions[0].windows[1].id, "@75");
        assert!(sessions[0].windows[1].active);
        assert_eq!(sessions[1].windows[0].name, "zsh");
    }

    #[test]
    fn each_tab_has_its_own_view() {
        assert_eq!(view_name("web", 1), "web·signalbox");
        assert_eq!(view_name("web", 2), "web·signalbox2");
        assert_eq!(viewed_session("web·signalbox"), Some("web"));
        assert_eq!(viewed_session("web·signalbox12"), Some("web"));
        assert_eq!(viewed_session("web"), None);
        assert_eq!(viewed_session("web·signalboxes"), None);
        assert_eq!(
            search_args("web·signalbox2", &Search::Find("panic".into())).join(" "),
            "copy-mode -t =web·signalbox2: ; send-keys -t =web·signalbox2: -X search-backward-incremental =panic"
        );
        assert_eq!(
            search_args("web·signalbox", &Search::Done).join(" "),
            "send-keys -t =web·signalbox: -X cancel"
        );
        let (_, args) = attach_command("web", "web·signalbox2", Some("@75"));
        assert_eq!(args[..3], ["-u", "-T", "hyperlinks"]);
        assert_eq!(args[3..9], ["new-session", "-A", "-s", "web·signalbox2", "-t", "=web"]);
        assert_eq!(args.last().map(String::as_str), Some("=web·signalbox2:@75"));
    }
}
