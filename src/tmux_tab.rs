//! A tab showing one tmux session: the terminal, and beside it what the terminal can't show,
//! the images and videos the Claude session on screen made or looked at.
//!
//! The tab runs its tmux client while the session exists, as the watcher sees tmux: it waits
//! while the session is gone or being rebuilt, and attaches by itself when it's back.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, ObjectFit, SharedString, Subscription, Window, div, img, prelude::*, px, rgb, rgba,
};

use gpui::{KeyBinding, actions};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable};

use crate::media::{Kind, MediaItem};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme;
use crate::tmux;
use crate::ui;
use crate::watch::Watch;
use crate::live::ClaudeState;
use crate::tmux::TmuxWindow;
use crate::watcher::{self, Machine, Watcher};

actions!(tmux_tab, [Find, TogglePanel]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("cmd-b", TogglePanel, None),
    ]);
}

/// The views the app's tabs hold, so each tab gets its own.
static VIEWS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The first view of `session` that no tab holds, now held.
fn claim_view(session: &str) -> String {
    let mut held = VIEWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let view = (1..)
        .map(|number| tmux::view_name(session, number))
        .find(|view| !held.contains(view))
        .expect("there's always a next number");
    held.push(view.clone());
    view
}

fn release_view(view: &str) {
    let mut held = VIEWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    held.retain(|held| held != view);
}

/// Where the tab's tmux client stands.
enum Link {
    /// tmux hasn't been read yet.
    Unknown,
    /// The session isn't running, or is being rebuilt.
    Waiting,
    Attached,
    /// The client ended while the session kept running (detached), or couldn't start.
    Ended(String),
}

pub struct TmuxTab {
    session: String,
    /// This tab's own view of the session (see `tmux::view_name`).
    view: String,
    /// The window to open on: the one asked for, then wherever the view was last. Window ids
    /// don't outlive their session, so it's forgotten while the session is gone.
    window_id: Option<String>,
    link: Link,
    terminal: Entity<TerminalView>,
    watcher: Entity<Watcher>,
    watch: Option<Entity<Watch>>,
    /// `None` shows the panel whenever there is media; Cmd+B makes it the person's choice.
    panel: Option<bool>,
    /// The image open large over the tab.
    viewing: Option<PathBuf>,
    viewer_focus: FocusHandle,
    /// The find bar's field while it's open (Cmd-F).
    search: Option<(Entity<InputState>, Subscription)>,
    /// Search steps for tmux, run one after another off the main thread.
    search_steps: async_channel::Sender<tmux::Search>,
    _subscriptions: Vec<Subscription>,
}

impl TmuxTab {
    pub fn new(session: String, window_id: Option<&str>, cx: &mut Context<Self>) -> Self {
        let watcher = watcher::get(cx);
        let view = claim_view(&session);
        let (search_steps, steps) = async_channel::unbounded::<tmux::Search>();
        let searched = view.clone();
        cx.background_executor()
            .spawn(async move {
                while let Ok(step) = steps.recv().await {
                    let args = tmux::search_args(&searched, &step);
                    tmux::run(&args.iter().map(String::as_str).collect::<Vec<_>>());
                }
            })
            .detach();
        let terminal = cx.new(TerminalView::new);
        let subscriptions = vec![
            cx.subscribe(&terminal, |tab, _, event: &TerminalEvent, cx| {
                if matches!(event, TerminalEvent::Exited) {
                    tab.exited(cx);
                }
                cx.notify();
            }),
            cx.observe(&watcher, |tab, _, cx| tab.sync(cx)),
        ];
        let mut tab = Self {
            session,
            view,
            window_id: window_id.map(str::to_string),
            link: Link::Unknown,
            terminal,
            watcher,
            watch: None,
            panel: None,
            viewing: None,
            viewer_focus: cx.focus_handle(),
            search: None,
            search_steps,
            _subscriptions: subscriptions,
        };
        tab.sync(cx);
        tab
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn view(&self) -> &str {
        &self.view
    }

    /// The window on screen, as tmux last said (or the one asked for, before it has).
    pub fn window_id(&self) -> Option<&str> {
        self.window_id.as_deref()
    }

    /// The window on screen, as tmux was last read.
    fn shown<'a>(&self, machine: &'a Machine) -> Option<&'a TmuxWindow> {
        match self.window_id.as_deref() {
            Some(id) => machine
                .sessions
                .iter()
                .find(|session| session.name == self.session)?
                .windows
                .iter()
                .find(|window| window.id == id),
            None => machine.shown_window(&self.session, &self.view),
        }
    }

    /// "session — window", as tmux was last read.
    pub fn title(&self, cx: &App) -> SharedString {
        let machine = self.watcher.read(cx).machine.as_ref();
        match machine.and_then(|machine| self.shown(machine)) {
            Some(window) => format!("{} — {}", self.session, window.name).into(),
            None => self.session.clone().into(),
        }
    }

    /// The state of the Claude session in the window on screen, if one runs there.
    pub fn state(&self, cx: &App) -> Option<ClaudeState> {
        let machine = self.watcher.read(cx).machine.as_ref()?;
        machine.window_state(self.shown(machine)?)
    }

    /// Brings the tmux client in line with tmux: attach once the session runs, wait while it
    /// doesn't. A client that was detached stays so until Reattach.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let watcher = self.watcher.read(cx);
        let ready = watcher.session_ready(&self.session);
        if matches!(self.link, Link::Attached)
            && let Some(window) = watcher.machine.as_ref().and_then(|machine| machine.views.get(&self.view))
        {
            self.window_id = Some(window.clone());
        }
        match (&self.link, ready) {
            (Link::Attached, _) | (_, None) | (Link::Ended(_), Some(true)) => {}
            (Link::Waiting, Some(false)) => {}
            (_, Some(false)) => {
                self.link = Link::Waiting;
                // The session will come back with new window ids.
                self.window_id = None;
                cx.notify();
            }
            (Link::Unknown | Link::Waiting, Some(true)) => self.attach(cx),
        }
        self.follow_window(cx);
    }

    /// Runs the tmux client, on the window last shown, or else the one the session is on (a new
    /// view would otherwise open on the session's first window).
    fn attach(&mut self, cx: &mut Context<Self>) {
        let window = self.window_id.clone().or_else(|| {
            let machine = self.watcher.read(cx).machine.as_ref()?;
            Some(machine.shown_window(&self.session, &self.view)?.id.clone())
        });
        let (program, args) = tmux::attach_command(&self.session, &self.view, window.as_deref());
        let started = self.terminal.update(cx, |terminal, cx| terminal.start(&program, &args, cx));
        self.link = match started {
            Ok(()) => Link::Attached,
            Err(error) => Link::Ended(format!("Couldn't start tmux: {error}")),
        };
        cx.notify();
    }

    fn exited(&mut self, cx: &mut Context<Self>) {
        self.link = match self.watcher.read(cx).session_ready(&self.session) {
            Some(true) => Link::Ended(format!("Detached from {}.", self.session)),
            _ => Link::Waiting,
        };
    }

    fn reattach(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.attach(cx);
        let handle = self.terminal.focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Cmd-F: a find bar over the terminal, searching the pane's whole history in tmux.
    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.link, Link::Attached) {
            return;
        }
        if self.search.is_none() {
            let field = cx.new(|cx| InputState::new(window, cx).placeholder("Find in this session"));
            let subscription = cx.subscribe_in(&field, window, |tab, field, event: &InputEvent, _, cx| {
                let step = match event {
                    InputEvent::Change => tmux::Search::Find(field.read(cx).value().to_string()),
                    InputEvent::PressEnter { shift: true, .. } => tmux::Search::Newer,
                    InputEvent::PressEnter { .. } => tmux::Search::Older,
                    _ => return,
                };
                let _ = tab.search_steps.try_send(step);
            });
            self.search = Some((field, subscription));
        }
        if let Some((field, _)) = &self.search {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
        cx.notify();
    }

    /// Closes the find bar and takes the pane back to live output.
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            let _ = self.search_steps.try_send(tmux::Search::Done);
        }
        let handle = self.terminal.focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// A find bar, as in Safari: Enter for the next (older) match, Shift-Enter for the one
    /// before, Escape to close. The field passes Escape on when it has nothing of its own to
    /// dismiss.
    fn search_bar(&self, field: &Entity<InputState>, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("find-bar")
            .absolute()
            .top(px(12.))
            .right(px(16.))
            .w(px(380.))
            .flex()
            .items_center()
            .gap_2()
            .p(px(6.))
            .rounded(px(10.))
            .bg(rgb(theme::SURFACE))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .shadow_lg()
            .occlude()
            .on_key_down(cx.listener(|tab, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    tab.close_search(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                Input::new(field)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).small().text_color(rgb(theme::TEXT_FAINT))),
            )
            .child(
                div()
                    .flex_none()
                    .pr(px(6.))
                    .text_size(px(11.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child("↵ ⇧↵"),
            )
            .into_any_element()
    }

    /// What covers the terminal while no client runs in it.
    fn link_note(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let unreachable = self.watcher.read(cx).tmux_error.clone();
        match &self.link {
            Link::Attached => None,
            Link::Unknown => unreachable.map(|error| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_sm()
                    .child(div().text_color(rgb(theme::TEXT)).child("Can't reach tmux"))
                    .child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child(error))
                    .into_any_element()
            }),
            Link::Waiting => {
                let rebuilding = self.watcher.read(cx).rebuilding(&self.session);
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_sm()
                        .child(div().text_color(rgb(theme::TEXT)).child(if rebuilding {
                            format!("Rebuilding {}…", self.session)
                        } else {
                            format!("{} isn't running", self.session)
                        }))
                        .child(div().text_xs().text_color(rgb(theme::TEXT_MUTED)).child(if rebuilding {
                            "It opens here when it's ready."
                        } else {
                            "It opens here when the session is back."
                        }))
                        .into_any_element(),
                )
            }
            Link::Ended(note) => Some(
                div()
                    .absolute()
                    .bottom_4()
                    .left_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(theme::SURFACE))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .text_sm()
                    .text_color(rgb(theme::TEXT))
                    .child(note.clone())
                    .child(
                        ui::button("reattach", "Reattach", ui::ButtonKind::Primary)
                            .on_click(cx.listener(Self::reattach)),
                    )
                    .into_any_element(),
            ),
        }
    }

    /// Watch the Claude session in the window now on screen, if there is one.
    fn follow_window(&mut self, cx: &mut Context<Self>) {
        let target = self
            .watcher
            .read(cx)
            .machine
            .as_ref()
            .and_then(|machine| machine.claude_in_view(&self.session, &self.view));
        match (target, &self.watch) {
            (None, None) => {}
            (None, Some(_)) => {
                self.watch = None;
                cx.notify();
            }
            (Some(session), Some(watch)) if watch.read(cx).session.session_id == session.session_id => {
                watch.update(cx, |watch, cx| watch.update_session(session, cx));
            }
            (Some(session), _) => {
                let watch = cx.new(|cx| Watch::new(session, cx));
                self._subscriptions.push(cx.observe(&watch, |_, _, cx| cx.notify()));
                self.watch = Some(watch);
                cx.notify();
            }
        }
    }

    /// The images and videos the Claude session on screen made or looked at, newest first.
    pub fn media(&self, cx: &App) -> Vec<MediaItem> {
        self.watch
            .as_ref()
            .map(|watch| watch.read(cx).media.clone())
            .unwrap_or_default()
    }

    /// Whether the media column shows for this tab: whenever there is media, unless Cmd-B said
    /// otherwise.
    pub fn panel_shown(&self, cx: &App) -> bool {
        self.panel.unwrap_or_else(|| !self.media(cx).is_empty())
    }

    fn toggle_panel(&mut self, _: &TogglePanel, _: &mut Window, cx: &mut Context<Self>) {
        self.panel = Some(!self.panel_shown(cx));
        cx.notify();
    }

    /// Opens an image over the terminal, or a video in QuickTime.
    pub fn open(&mut self, item: &MediaItem, window: &mut Window, cx: &mut Context<Self>) {
        match item.kind {
            Kind::Video => cx.open_with_system(&item.path),
            Kind::Image => {
                self.viewing = Some(item.path.clone());
                window.focus(&self.viewer_focus, cx);
                cx.notify();
            }
        }
    }

    fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewing = None;
        let handle = self.terminal.focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Steps to the next or previous image in the panel's order.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let images: Vec<PathBuf> = self
            .media(cx)
            .into_iter()
            .filter(|item| item.kind == Kind::Image)
            .map(|item| item.path)
            .collect();
        let Some(current) = &self.viewing else { return };
        let Some(ix) = images.iter().position(|path| path == current) else {
            return;
        };
        let next = ix as isize + by;
        if (0..images.len() as isize).contains(&next) {
            self.viewing = Some(images[next as usize].clone());
            cx.notify();
        }
    }

    fn viewer_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" | "space" => self.close_viewer(window, cx),
            "left" | "up" => self.step(-1, cx),
            "right" | "down" => self.step(1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn viewer(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let (open, reveal) = (path.to_path_buf(), path.to_path_buf());
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .bg(rgba(0x0000_00e6))
            .occlude()
            .track_focus(&self.viewer_focus)
            .on_key_down(cx.listener(Self::viewer_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|tab, _: &MouseDownEvent, window, cx| tab.close_viewer(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(rgb(theme::TEXT))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::TEXT_FAINT))
                            .child("← → to step · esc"),
                    )
                    .child(
                        ui::button("viewer-open", "Open", ui::ButtonKind::Quiet)
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.open_with_system(&open))),
                    )
                    .child(
                        ui::button("viewer-reveal", "Show in Finder", ui::ButtonKind::Quiet)
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.reveal_path(&reveal))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .child(img(path.to_path_buf()).size_full().object_fit(ObjectFit::Contain)),
            )
            .into_any_element()
    }
}

impl Render for TmuxTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search = self.search.as_ref().map(|(field, _)| field.clone()).map(|field| self.search_bar(&field, cx));
        let viewer = self.viewing.clone().map(|path| self.viewer(&path, cx));
        let note = self.link_note(cx);
        div()
            .relative()
            .size_full()
            .flex()
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::find))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.terminal.clone())
                    .children(note)
                    .children(search),
            )
            .children(viewer)
    }
}

impl Drop for TmuxTab {
    fn drop(&mut self) {
        release_view(&self.view);
    }
}

impl Focusable for TmuxTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal.focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Entity, KeyDownEvent, Render, TestAppContext, Window, div, prelude::*};
    use gpui_kit::component::input::{Input, InputState};

    /// The find bar relies on this: with nothing of its own to dismiss, the kit's field
    /// passes Escape on to the key handlers around it.
    struct Bar {
        field: Entity<InputState>,
        escaped: bool,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .on_key_down(cx.listener(|bar, event: &KeyDownEvent, _, _| {
                    if event.keystroke.key == "escape" {
                        bar.escaped = true;
                    }
                }))
                .child(Input::new(&self.field))
        }
    }

    #[test]
    fn each_tab_of_a_session_gets_the_next_free_view() {
        let first = super::claim_view("views-test");
        let second = super::claim_view("views-test");
        assert_eq!(first, "views-test·signalbox");
        assert_eq!(second, "views-test·signalbox2");
        super::release_view(&first);
        assert_eq!(super::claim_view("views-test"), "views-test·signalbox", "a closed tab's view is free again");
    }

    #[gpui::test]
    fn the_field_passes_escape_to_the_find_bar(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (bar, cx) = cx.add_window_view(|window, cx| Bar {
            field: cx.new(|cx| InputState::new(window, cx)),
            escaped: false,
        });
        bar.update_in(cx, |bar, window, cx| bar.field.update(cx, |field, cx| field.focus(window, cx)));
        cx.run_until_parked();
        cx.simulate_input("panic");
        assert_eq!(bar.read_with(cx, |bar, cx| bar.field.read(cx).value().to_string()), "panic");
        cx.simulate_keystrokes("escape");
        assert!(bar.read_with(cx, |bar, _| bar.escaped));
    }
}
