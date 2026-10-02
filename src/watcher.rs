//! What the app knows about this machine, kept for as long as it runs, with or without a
//! window: the tmux sessions and the Claude sessions in them, read every two seconds. It tells
//! you when a session needs you, saves workspace snapshots, and after the Mac restarts brings
//! the workspace back by itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, Global, Task};

use crate::alerts;
use crate::live::{self, ClaudeState, LiveSession};
use crate::status::SessionStatus;
use crate::snapshots::{self, Plan, Workspace};
use crate::tmux::{self, TmuxSession, TmuxWindow};

const POLL: Duration = Duration::from_secs(2);
/// Snapshots are written at most this often, and only when something changed.
const SAVE_EVERY: u64 = 5 * 60;

fn rebuilt_marker() -> PathBuf {
    crate::sys::data_dir().join("rebuilt-for-boot")
}

/// Whether the workspace was already rebuilt in the boot `boot_id`.
fn rebuilt_in(boot_id: &str) -> bool {
    std::fs::read_to_string(rebuilt_marker()).is_ok_and(|marked| marked.trim() == boot_id)
}

fn mark_rebuilt(boot_id: &str) {
    let _ = std::fs::write(rebuilt_marker(), boot_id);
}

/// The workspace to bring back by itself after the Mac restarts: the snapshot saved before
/// this boot, once per boot (by its id). Test copies never rebuild.
pub fn pending_rebuild() -> Option<(String, Workspace)> {
    if crate::sys::dev_mode() {
        return None;
    }
    let boot_id = crate::sys::boot_id()?;
    if rebuilt_in(&boot_id) {
        return None;
    }
    let workspace = snapshots::from_before(&snapshots::dir(), crate::sys::boot_time()?)?;
    Some((boot_id, workspace))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Windows by the state of their Claude session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tallies {
    pub needs_you: usize,
    pub working: usize,
    pub idle: usize,
}

#[derive(Clone, PartialEq)]
pub struct Machine {
    /// The person's sessions; the app's own views are folded into `views`.
    pub sessions: Vec<TmuxSession>,
    /// The window each of the app's views shows, by the view's name.
    pub views: HashMap<String, String>,
    /// Claude sessions by tmux pane id.
    pub claude: HashMap<String, LiveSession>,
}

impl Machine {
    fn read() -> Result<Self, String> {
        let tmux = tmux::sessions()?;
        let claude = live::scan()
            .into_iter()
            .filter_map(|session| Some((session.pane.clone()?, session)))
            .collect();
        let mut views = HashMap::new();
        let mut sessions = Vec::new();
        for session in tmux {
            match tmux::viewed_session(&session.name) {
                Some(_) => {
                    if let Some(window) = session.windows.iter().find(|window| window.active) {
                        views.insert(session.name.clone(), window.id.clone());
                    }
                }
                None => sessions.push(session),
            }
        }
        Ok(Self {
            sessions,
            views,
            claude,
        })
    }

    pub fn running(&self) -> Vec<String> {
        self.sessions.iter().map(|session| session.name.clone()).collect()
    }

    /// The Claude session in a window (the most urgent one if several share it).
    pub fn window_claude(&self, window: &TmuxWindow) -> Option<&LiveSession> {
        window
            .panes
            .iter()
            .filter_map(|pane| self.claude.get(&pane.id))
            .fold(None::<&LiveSession>, |most, session| match most {
                Some(most)
                    if ClaudeState::most_urgent(Some(most.state.clone()), session.state.clone()) == most.state =>
                {
                    Some(most)
                }
                _ => Some(session),
            })
    }

    pub fn window_state(&self, window: &TmuxWindow) -> Option<ClaudeState> {
        self.window_claude(window).map(|session| session.state.clone())
    }

    /// The Claude session in the window a view of `session` shows.
    pub fn claude_in_view(&self, session: &str, view: &str) -> Option<LiveSession> {
        self.window_claude(self.shown_window(session, view)?).cloned()
    }

    /// How many windows' Claude sessions need you, are working, or sit idle.
    pub fn tallies(&self) -> Tallies {
        let mut tallies = Tallies::default();
        for window in self.sessions.iter().flat_map(|session| session.windows.iter()) {
            match self.window_state(window) {
                Some(ClaudeState::NeedsYou(_)) => tallies.needs_you += 1,
                Some(ClaudeState::Working) => tallies.working += 1,
                Some(ClaudeState::Shell | ClaudeState::Idle) => tallies.idle += 1,
                None => {}
            }
        }
        tallies
    }

    /// The account's usage limits, as the freshest status line reported them.
    pub fn limits(&self) -> Option<&SessionStatus> {
        self.claude
            .values()
            .filter_map(|session| session.status.as_ref())
            .filter(|status| status.five_hour.is_some() || status.seven_day.is_some())
            .max_by_key(|status| status.updated)
    }

    /// The window a view of `session` shows, or the session's own active window while the
    /// view isn't running.
    pub fn shown_window(&self, session: &str, view: &str) -> Option<&TmuxWindow> {
        let found = self.sessions.iter().find(|s| s.name == session)?;
        match self.views.get(view) {
            Some(id) => found.windows.iter().find(|window| &window.id == id),
            None => found.windows.iter().find(|window| window.active),
        }
    }

    /// The window each viewed session reopens on after a rebuild: what its first view shows.
    pub fn session_views(&self) -> HashMap<String, String> {
        let mut views: Vec<(&String, &String)> = self.views.iter().collect();
        views.sort();
        let mut shown = HashMap::new();
        for (view, window) in views {
            if let Some(session) = tmux::viewed_session(view) {
                shown.entry(session.to_string()).or_insert_with(|| window.clone());
            }
        }
        shown
    }
}

/// A Claude session as a poll saw it, and where it is.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    state: ClaudeState,
    name: String,
    session: String,
    window_id: String,
    /// The window's index and name, as the sidebar shows it.
    window: String,
}

/// What a poll calls for.
#[derive(Debug, PartialEq)]
enum Alert {
    /// Started waiting on you.
    NeedsYou(Seen),
    /// Finished its turn: working at one poll, idle at the next two.
    Finished(Seen),
    /// A notification of this kind for this session no longer applies.
    Clear(alerts::Kind, Seen),
}

/// Claude sessions from poll to poll, and what changed that's worth telling you.
#[derive(Default)]
struct Attention {
    /// Every Claude session at the last poll, by session id (`None` before the first).
    seen: Option<HashMap<String, Seen>>,
    /// Stopped working at the last poll: finished if still idle at this one. A turn can pause
    /// between steps; one poll of quiet tells the two apart.
    finishing: HashMap<String, Seen>,
    /// Said to have finished; cleared once they work again.
    finished: HashMap<String, Seen>,
}

impl Attention {
    fn update(&mut self, now: HashMap<String, Seen>) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let candidates = std::mem::take(&mut self.finishing);
        let Some(before) = self.seen.replace(now.clone()) else {
            // Sessions already waiting or idle at launch only count.
            return alerts;
        };
        let state = |id: &String| now.get(id).map(|seen| &seen.state);
        for (id, was) in &before {
            if matches!(was.state, ClaudeState::NeedsYou(_)) && !matches!(state(id), Some(ClaudeState::NeedsYou(_))) {
                alerts.push(Alert::Clear(alerts::Kind::NeedsYou, was.clone()));
            }
        }
        self.finished.retain(|id, told| {
            let idle = state(id) == Some(&ClaudeState::Idle);
            if !idle {
                alerts.push(Alert::Clear(alerts::Kind::Finished, told.clone()));
            }
            idle
        });
        for (id, seen) in &now {
            let was = before.get(id).map(|before| &before.state);
            match &seen.state {
                ClaudeState::NeedsYou(_) if !matches!(was, Some(ClaudeState::NeedsYou(_))) => {
                    alerts.push(Alert::NeedsYou(seen.clone()));
                }
                ClaudeState::Idle if candidates.contains_key(id) => {
                    self.finished.insert(id.clone(), seen.clone());
                    alerts.push(Alert::Finished(seen.clone()));
                }
                ClaudeState::Idle if was == Some(&ClaudeState::Working) => {
                    self.finishing.insert(id.clone(), seen.clone());
                }
                _ => {}
            }
        }
        alerts
    }
}

pub enum Restoring {
    Idle,
    /// Rebuilding these sessions.
    Running(Vec<String>),
    Done { title: String, lines: Vec<String> },
}

pub struct Watcher {
    /// tmux and its Claude sessions as last read.
    pub machine: Option<Machine>,
    /// Why the last read of tmux failed; `machine` keeps the read before it.
    pub tmux_error: Option<String>,
    pub restoring: Restoring,
    /// What Claude sessions were doing at the last polls, for notifications.
    attention: Attention,
    /// The count on the Dock icon.
    badge: Option<usize>,
    last_save_attempt: u64,
    /// Whether this run has looked for a workspace to rebuild after a restart.
    checked_boot: bool,
    _poll: Task<()>,
}

struct GlobalWatcher(Entity<Watcher>);

impl Global for GlobalWatcher {}

pub fn init(cx: &mut App) {
    let watcher = cx.new(Watcher::new);
    cx.set_global(GlobalWatcher(watcher));
}

/// A watcher that never reads tmux, for tests.
#[cfg(test)]
pub fn init_idle(cx: &mut App) {
    let watcher = cx.new(|_| Watcher {
        machine: None,
        tmux_error: None,
        restoring: Restoring::Idle,
        attention: Attention::default(),
        badge: None,
        last_save_attempt: now(),
        checked_boot: true,
        _poll: Task::ready(()),
    });
    cx.set_global(GlobalWatcher(watcher));
}

pub fn get(cx: &App) -> Entity<Watcher> {
    cx.global::<GlobalWatcher>().0.clone()
}

impl Watcher {
    fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let machine = cx.background_executor().spawn(async { Machine::read() }).await;
                if this.update(cx, |watcher, cx| watcher.update(machine, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            machine: None,
            tmux_error: None,
            restoring: Restoring::Idle,
            attention: Attention::default(),
            badge: None,
            // Wait a minute before the first save: right after a restart tmux is half built.
            last_save_attempt: now().saturating_sub(SAVE_EVERY) + 60,
            checked_boot: false,
            _poll: poll,
        }
    }

    /// Reads tmux now rather than at the next poll.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let machine = cx.background_executor().spawn(async { Machine::read() }).await;
            this.update(cx, |watcher, cx| watcher.update(machine, cx)).ok();
        })
        .detach();
    }

    /// Whether `session` is running and finished being built: `None` while tmux can't be read.
    pub fn session_ready(&self, session: &str) -> Option<bool> {
        if self.tmux_error.is_some() {
            return None;
        }
        let machine = self.machine.as_ref()?;
        Some(machine.sessions.iter().any(|s| s.name == session) && !self.rebuilding(session))
    }

    pub fn rebuilding(&self, session: &str) -> bool {
        matches!(&self.restoring, Restoring::Running(sessions) if sessions.iter().any(|s| s == session))
    }

    fn update(&mut self, read: Result<Machine, String>, cx: &mut Context<Self>) {
        // A failed read changes nothing: taken as "no sessions" it would clear every alert and
        // rebuild sessions that are running.
        let machine = match read {
            Ok(machine) => machine,
            Err(error) => {
                if self.tmux_error.as_ref() != Some(&error) {
                    eprintln!("[watcher] couldn't read tmux: {error}");
                    self.tmux_error = Some(error);
                    cx.notify();
                }
                return;
            }
        };
        if self.tmux_error.take().is_some() {
            cx.notify();
        }
        let dev = crate::sys::dev_mode();
        if !dev {
            self.alert(&machine);
        }
        self.rebuild_after_reboot(&machine, cx);
        if !dev && now() >= self.last_save_attempt + SAVE_EVERY {
            self.save(&machine, cx);
        }
        if self.machine.as_ref() != Some(&machine) {
            self.machine = Some(machine);
            cx.notify();
        }
    }

    /// File > Save Workspace Now.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        if crate::sys::dev_mode() {
            self.report("Not saved", "Test copies don't save workspaces.", cx);
            return;
        }
        if let Some(machine) = self.machine.clone() {
            self.save(&machine, cx);
        }
    }

    fn save(&mut self, machine: &Machine, cx: &mut Context<Self>) {
        let stamp = now();
        self.last_save_attempt = stamp;
        // Half-rebuilt sessions aren't worth keeping; the next save comes after.
        if machine.sessions.is_empty() || matches!(self.restoring, Restoring::Running(_)) {
            return;
        }
        let machine = machine.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    let workspace = snapshots::capture(&machine.sessions, &machine.session_views(), &machine.claude, stamp);
                    snapshots::save(&snapshots::dir(), &workspace)
                })
                .await;
            if let Err(error) = saved {
                eprintln!("[workspace] couldn't save: {error}");
            }
            this.update(cx, |_, _| {}).ok();
        })
        .detach();
    }

    /// Once after the Mac restarts, rebuilds the sessions from before that aren't running.
    /// Closing sessions afterwards, even the last one (which stops tmux), keeps them closed.
    fn rebuild_after_reboot(&mut self, machine: &Machine, cx: &mut Context<Self>) {
        if self.checked_boot {
            return;
        }
        self.checked_boot = true;
        let running = machine.running();
        cx.spawn(async move |this, cx| {
            let Some((boot_id, workspace)) = cx.background_executor().spawn(async { pending_rebuild() }).await
            else {
                return;
            };
            let plan = snapshots::plan(&workspace, &running);
            if plan.create.is_empty() {
                mark_rebuilt(&boot_id);
                return;
            }
            this.update(cx, |watcher, cx| watcher.run_restore(workspace, plan, Some(boot_id), cx))
                .ok();
        })
        .detach();
    }

    /// File > Restore Workspace: rebuilds what's missing from the newest snapshot.
    pub fn restore_latest(&mut self, cx: &mut Context<Self>) {
        if matches!(self.restoring, Restoring::Running(_)) {
            return;
        }
        if crate::sys::dev_mode() {
            self.report("Not restored", "Test copies don't restore workspaces.", cx);
            return;
        }
        let running = self.machine.as_ref().map(Machine::running).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let latest = cx
                .background_executor()
                .spawn(async { snapshots::list(&snapshots::dir()).first().and_then(|path| snapshots::load(path)) })
                .await;
            this.update(cx, |watcher, cx| {
                let Some(workspace) = latest else {
                    watcher.report("Nothing to restore", "No workspace has been saved yet.", cx);
                    return;
                };
                let plan = snapshots::plan(&workspace, &running);
                if plan.create.is_empty() {
                    watcher.report(
                        "Nothing to restore",
                        "Everything in the last saved workspace is running.",
                        cx,
                    );
                    return;
                }
                watcher.run_restore(workspace, plan, None, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Rebuilds the sessions in `plan`. After a restart (`after_boot`, that boot's id), it also
    /// remembers the boot so it happens once, and says so in a notification: nobody may be
    /// looking yet. Tabs for these sessions wait until it's done, then attach.
    fn run_restore(&mut self, workspace: Workspace, plan: Plan, after_boot: Option<String>, cx: &mut Context<Self>) {
        let running = self.machine.as_ref().map(Machine::running).unwrap_or_default();
        let sessions = plan.create.len();
        let claudes: usize = plan.create.iter().map(|session| session.claudes).sum();
        let automatic = after_boot.is_some();
        self.restoring = Restoring::Running(plan.create.iter().map(|session| session.name.clone()).collect());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let lines = cx
                .background_executor()
                .spawn(async move {
                    let lines = snapshots::restore(&workspace, &running);
                    if let Some(boot_id) = after_boot {
                        mark_rebuilt(&boot_id);
                    }
                    lines
                })
                .await;
            let summary = format!(
                "{sessions} tmux session{} rebuilt, {claudes} Claude conversation{} resumed.",
                if sessions == 1 { "" } else { "s" },
                if claudes == 1 { "" } else { "s" }
            );
            if automatic {
                alerts::notify_info("Workspace restored", &summary);
            }
            this.update(cx, |watcher, cx| {
                watcher.restoring = Restoring::Done {
                    title: "Workspace restored".to_string(),
                    lines,
                };
                cx.notify();
                watcher.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn report(&mut self, title: &str, line: &str, cx: &mut Context<Self>) {
        self.restoring = Restoring::Done {
            title: title.to_string(),
            lines: vec![line.to_string()],
        };
        cx.notify();
    }

    pub fn clear_report(&mut self, cx: &mut Context<Self>) {
        self.restoring = Restoring::Idle;
        cx.notify();
    }

    /// Marks the window one of the app's views shows right away, instead of waiting for the
    /// next poll.
    pub fn mark_active(&mut self, view: &str, window_id: &str, cx: &mut Context<Self>) {
        if let Some(machine) = &mut self.machine {
            machine.views.insert(view.to_string(), window_id.to_string());
            cx.notify();
        }
    }

    /// Notifies about Claude sessions that started waiting on you (Glass) or finished their turn
    /// (Pop; not while you're looking at that window), clears notifications that no longer
    /// apply, and keeps the Dock badge at the count waiting on you.
    fn alert(&mut self, machine: &Machine) {
        let mut now = HashMap::new();
        for session in &machine.sessions {
            for window in &session.windows {
                for pane in &window.panes {
                    if let Some(claude) = machine.claude.get(&pane.id) {
                        now.insert(
                            claude.session_id.clone(),
                            Seen {
                                state: claude.state.clone(),
                                name: claude.name.clone(),
                                session: session.name.clone(),
                                window_id: window.id.clone(),
                                window: format!("{} {}", window.index, window.name),
                            },
                        );
                    }
                }
            }
        }
        let waiting = now
            .values()
            .filter(|seen| matches!(seen.state, ClaudeState::NeedsYou(_)))
            .count();
        for alert in self.attention.update(now) {
            match alert {
                Alert::NeedsYou(seen) => {
                    let what = match &seen.state {
                        ClaudeState::NeedsYou(what) => what.as_deref(),
                        _ => None,
                    };
                    alerts::notify(
                        alerts::Kind::NeedsYou,
                        &seen.name,
                        &format!("{} · {}", seen.session, seen.window),
                        &format!("Waiting for you: {}", what.unwrap_or("an answer")),
                        &seen.session,
                        &seen.window_id,
                    );
                }
                Alert::Finished(seen) if !alerts::watching(&seen.session, &seen.window_id) => {
                    alerts::notify(
                        alerts::Kind::Finished,
                        &seen.name,
                        &format!("{} · {}", seen.session, seen.window),
                        "Finished. Your turn.",
                        &seen.session,
                        &seen.window_id,
                    );
                }
                Alert::Finished(_) => {}
                Alert::Clear(kind, seen) => alerts::clear(kind, &seen.session, &seen.window_id),
            }
        }
        if self.badge != Some(waiting) {
            alerts::set_badge(waiting);
            self.badge = Some(waiting);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(state: ClaudeState) -> HashMap<String, Seen> {
        let seen = Seen {
            state,
            name: "Planner".into(),
            session: "web".into(),
            window_id: "@2".into(),
            window: "2 api".into(),
        };
        HashMap::from([("s1".to_string(), seen)])
    }

    fn told(alerts: &[Alert]) -> Vec<&'static str> {
        alerts
            .iter()
            .map(|alert| match alert {
                Alert::NeedsYou(_) => "needs you",
                Alert::Finished(_) => "finished",
                Alert::Clear(alerts::Kind::NeedsYou, _) => "clear needs you",
                Alert::Clear(alerts::Kind::Finished, _) => "clear finished",
                Alert::Clear(alerts::Kind::Info, _) => "clear info",
            })
            .collect()
    }

    #[test]
    fn a_turn_is_finished_once_it_stays_quiet() {
        let mut attention = Attention::default();
        assert!(attention.update(one(ClaudeState::Working)).is_empty(), "the first poll only counts");
        assert!(attention.update(one(ClaudeState::Idle)).is_empty(), "one quiet poll could be a pause");
        assert_eq!(told(&attention.update(one(ClaudeState::Idle))), ["finished"]);
        assert!(attention.update(one(ClaudeState::Idle)).is_empty(), "said once");
        assert_eq!(told(&attention.update(one(ClaudeState::Working))), ["clear finished"]);
    }

    #[test]
    fn a_pause_mid_turn_isnt_finished() {
        let mut attention = Attention::default();
        attention.update(one(ClaudeState::Working));
        attention.update(one(ClaudeState::Idle));
        assert!(attention.update(one(ClaudeState::Working)).is_empty());
        assert!(attention.update(one(ClaudeState::Working)).is_empty());
    }

    #[test]
    fn waiting_on_you_is_told_then_cleared() {
        let mut attention = Attention::default();
        attention.update(one(ClaudeState::Working));
        let question = ClaudeState::NeedsYou(Some("input needed".into()));
        assert_eq!(told(&attention.update(one(question))), ["needs you"]);
        assert_eq!(told(&attention.update(one(ClaudeState::Working))), ["clear needs you"]);
    }

    #[test]
    fn sessions_waiting_or_idle_at_launch_only_count() {
        let mut attention = Attention::default();
        assert!(attention.update(one(ClaudeState::NeedsYou(None))).is_empty());
        assert!(attention.update(one(ClaudeState::NeedsYou(None))).is_empty());
        let mut idle = Attention::default();
        idle.update(one(ClaudeState::Idle));
        assert!(idle.update(one(ClaudeState::Idle)).is_empty());
    }
}
