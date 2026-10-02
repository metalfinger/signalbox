//! Every tmux window in the order you'd want to go to them: the ones that need you first, then
//! the working ones, then the rest in sidebar order. The Cmd-K palette, the start page and the
//! status bar all list from here.

use crate::live::ClaudeState;
use crate::watcher::Machine;

/// One tmux window, as the lists show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub session: String,
    pub window_id: String,
    pub label: String,
    pub claude: Option<String>,
    pub state: Option<ClaudeState>,
    pub waiting: Option<String>,
}

/// Every window, needs-you first, then working, then the rest in sidebar order.
pub fn entries(machine: &Machine) -> Vec<Entry> {
    let mut entries: Vec<Entry> = machine
        .sessions
        .iter()
        .flat_map(|session| {
            session.windows.iter().map(move |window| {
                let claude = machine.window_claude(window);
                let state = claude.map(|claude| claude.state.clone());
                Entry {
                    session: session.name.clone(),
                    window_id: window.id.clone(),
                    label: format!("{} {}", window.index, window.name),
                    claude: claude.map(|claude| claude.name.clone()),
                    waiting: match &state {
                        Some(ClaudeState::NeedsYou(what)) => {
                            Some(what.clone().unwrap_or_else(|| "needs you".to_string()))
                        }
                        _ => None,
                    },
                    state,
                }
            })
        })
        .collect();
    let rank = |entry: &Entry| match entry.state {
        Some(ClaudeState::NeedsYou(_)) => 0,
        Some(ClaudeState::Working) => 1,
        _ => 2,
    };
    entries.sort_by_key(rank);
    entries
}
