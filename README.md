# Signalbox

A native macOS app for your tmux sessions and the Claude Code sessions running in them. Every
tmux session and window sits in a sidebar with a light for the Claude session in it, so you can
see at a glance what's working and what's waiting on you, and open any of them in a real
terminal.

Signalbox shows and hosts; Claude does the work. It doesn't start, fork or drive agents itself.

## What it does

- **Every session in one place.** A sidebar lists each tmux session and its windows. A light
  shows the state of the Claude Code session in each window: working, needs you (with what it's
  waiting for), or idle, and how full its context is.
- **Real terminals.** Each tab attaches to a tmux session through its own grouped session, so
  choosing a window in Signalbox never moves or resizes your other terminals.
- **It tells you when you're needed.** A notification when a session starts waiting on you, and
  another when one finishes its turn (unless you're already looking at it), each with its own
  sound. Click one to go straight there. The Dock icon counts sessions waiting on you.
- **Go anywhere fast.** ⌘K jumps to any window by name, ⌘J to the next one that needs you.
- **The pictures beside the terminal.** A side panel collects the images and videos the Claude
  session on screen made or looked at, with a quick viewer.
- **Survives a restart.** Every few minutes Signalbox saves each session's windows, layout and
  folders, and the Claude conversation in each pane. After the Mac restarts it rebuilds the
  sessions that aren't running and resumes their conversations, once per boot.
- **A terminal that feels like a Mac app.** ⌘-click links, find in a pane's history, Mac line
  editing, press-and-hold accents and dictation, files dragged in as paths, images pasted as
  files Claude can read.

Everything stays on your Mac. Signalbox makes no network requests.

## Requirements

- macOS (built and used on Apple silicon)
- [tmux](https://github.com/tmux/tmux) 3.2 or later
- Rust 1.98 or later, to build it
- [Claude Code](https://docs.claude.com/en/docs/claude-code), for the session lights,
  notifications and resumes (Signalbox works as a tmux viewer without it)

## Install

```sh
git clone https://github.com/metalfinger/signalbox.git
cd signalbox
macos/install.sh
```

This builds Signalbox, installs `~/Applications/Signalbox.app` (open it from Spotlight or the
Dock) and a `signalbox` command in `~/.local/bin`. The first time it runs, Signalbox asks to send
notifications and adds itself to Login Items.

After pulling changes, run `cargo build --release && macos/update.sh`, or just `signalbox`,
which installs a newer build before opening.

### Context use and limits (optional)

To see each session's context use and your 5-hour and 7-day limits, use the status line in
`contrib/statusline.py` (or call it from your own). In `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "python3 /path/to/signalbox/contrib/statusline.py" }
```

## Using it

| Keys | What it does |
| --- | --- |
| ⌘K | Go to any tmux window |
| ⌘J | Go to the next window that needs you |
| ⌘T, ⌘W | New tab, close tab |
| ⌘1 to ⌘9, ⌃Tab | Switch tabs |
| ⌃⌘S | Show or hide the sidebar (drag its edge to resize it) |
| ⌘B | Show or hide the media panel |
| ⌘F | Find in the pane's history |
| ⌘+, ⌘−, ⌘0 | Text size |
| ⌘-click | Open a link |
| ⇧-drag, then ⌘C | Select and copy (a plain drag selects in tmux, which copies too) |
| ⌘V | Paste text, or an image as a file |
| ⌘← ⌘→, ⌘⌫ | Start or end of the line, delete the line |
| ⌥← ⌥→, ⌥⌫ | Move or delete by word |

File → Save Workspace Now and Restore Workspace save or rebuild the workspace on demand.

The `signalbox` command takes `--tmux NAME` to open a session in a new window, and `--go-to` to
open straight into ⌘K, handy for a global hotkey.

## How it works

- tmux is read with `list-panes` every two seconds. A tab attaches as `tmux new-session -t`,
  a grouped session named `<session>·signalbox` that tmux removes when the tab closes.
- Claude Code's own session files (`~/.claude/sessions/`) say which session runs in which pane
  and what it's doing. The side panel reads the session's transcript in `~/.claude/projects/`.
- Signalbox keeps its files in `~/.signalbox`: the window as you left it, preferences, workspace
  snapshots and status line reports.
- It's built with [GPUI](https://www.gpui.rs) and [GPUI Kit](https://gpui-kit.com); the
  terminal emulator is [alacritty_terminal](https://github.com/alacritty/alacritty).

## Developing

```sh
cargo test
cargo clippy --all-targets
```

`SIGNALBOX_DEV=1` runs a test copy that saves nothing, sends no notifications and leaves Login
Items alone. `SIGNALBOX_DEBUG=1` sends its log to the terminal instead of
`~/.signalbox/last-run.log`.

## License

MIT, see [LICENSE](LICENSE). The bundled Martian Mono font is under the SIL Open Font License
(`assets/fonts/OFL.txt`).
