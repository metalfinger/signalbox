use std::time::Duration;

use gpui::{
    Action, AnyElement, App, ClickEvent, Context, CursorStyle, DragMoveEvent, Entity, FocusHandle, Focusable,
    KeyBinding, MouseButton, MouseDownEvent, SharedString, Subscription, Task, Window, WindowBounds, actions, div,
    prelude::*, px, rgb, rgba,
};

use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{TitleBar, WindowExt};

use crate::live::ClaudeState;
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::start::{StartEvent, StartPage};
use crate::store::{self, SavedBounds, SavedTab, SavedWindow, WindowState};
use crate::switcher::{self, Entry};
use crate::theme;
use crate::tmux_tab::TmuxTab;
use crate::watcher;

actions!(
    shell,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ToggleSidebar,
        GoToWindow,
        NextNeedingYou
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = shell, no_json)]
pub struct ActivateTab(pub usize);

/// The sidebar's usual width, and how narrow or wide it can be dragged.
const SIDEBAR_WIDTH: f32 = 252.;
const SIDEBAR_WIDTHS: std::ops::RangeInclusive<f32> = 180.0..=420.0;

/// The sidebar's edge while it's dragged; it draws nothing under the pointer.
struct DraggedEdge;

impl Render for DraggedEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PreviousTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PreviousTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        // Cmd-0 is Actual Size, as in every Mac app; the sidebar takes Finder's shortcut.
        KeyBinding::new("ctrl-cmd-s", ToggleSidebar, None),
        KeyBinding::new("cmd-k", GoToWindow, None),
        KeyBinding::new("cmd-j", NextNeedingYou, None),
    ]);
    for n in 1..=9 {
        cx.bind_keys([KeyBinding::new(&format!("cmd-{n}"), ActivateTab(n - 1), None)]);
    }
}

pub enum Opening {
    Start,
    /// A tmux session, showing this window when given.
    Tmux {
        session: String,
        window: Option<String>,
    },
}

enum TabView {
    Start(Entity<StartPage>),
    /// A tmux client attached to `session`.
    Tmux(Entity<TmuxTab>),
}

struct Tab {
    id: usize,
    view: TabView,
    _subscription: Subscription,
}

impl Tab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.view {
            TabView::Start(page) => page.focus_handle(cx),
            TabView::Tmux(view) => view.focus_handle(cx),
        }
    }

    fn tmux(&self) -> Option<&Entity<TmuxTab>> {
        match &self.view {
            TabView::Tmux(view) => Some(view),
            _ => None,
        }
    }

    /// Whether this tab is on `session`, and on `window_id` when given.
    fn shows(&self, session: &str, window_id: Option<&str>, cx: &App) -> bool {
        self.tmux().is_some_and(|view| {
            let view = view.read(cx);
            view.session() == session && window_id.is_none_or(|id| view.window_id() == Some(id))
        })
    }
}

/// The window: the tmux sidebar, and a row of tabs (tmux sessions, and new tabs to pick one).
pub struct Shell {
    focus_handle: FocusHandle,
    sidebar: Entity<Sidebar>,
    show_sidebar: bool,
    sidebar_width: f32,
    tabs: Vec<Tab>,
    active: usize,
    next_id: usize,
    title: String,
    /// What was last written to ~/.signalbox/window.json.
    saved: Option<SavedWindow>,
    _save: Option<Task<()>>,
    _sidebar_events: Subscription,
    _watcher_changes: Subscription,
    _bounds_changes: Subscription,
}

impl Shell {
    pub fn new(openings: Vec<Opening>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let watcher = watcher::get(cx);
        let sidebar = cx.new(|cx| Sidebar::new(watcher.clone(), cx));
        let sidebar_events = cx.subscribe_in(&sidebar, window, |shell, _, event: &SidebarEvent, window, cx| {
            let SidebarEvent::Open { session, window_id } = event;
            shell.open_tmux(session, window_id.as_deref(), window, cx);
        });
        let watcher_changes = cx.observe_in(&watcher, window, |shell, _, window, cx| {
            cx.notify();
            shell.schedule_save(window, cx);
        });
        let bounds_changes = cx.observe_window_bounds(window, |shell, window, cx| shell.schedule_save(window, cx));
        let mut shell = Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            show_sidebar: true,
            sidebar_width: SIDEBAR_WIDTH,
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            title: String::new(),
            saved: None,
            _save: None,
            _sidebar_events: sidebar_events,
            _watcher_changes: watcher_changes,
            _bounds_changes: bounds_changes,
        };
        for opening in openings {
            let tab = shell.make_tab(opening, window, cx);
            shell.tabs.push(tab);
        }
        if shell.tabs.is_empty() {
            let tab = shell.make_tab(Opening::Start, window, cx);
            shell.tabs.push(tab);
        }
        shell
    }

    fn make_tab(&mut self, opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        match opening {
            Opening::Start => {
                let page = cx.new(StartPage::new);
                let subscription = cx.subscribe_in(&page, window, |shell, _, event: &StartEvent, window, cx| {
                    let StartEvent::OpenTmux { session, window_id } = event;
                    shell.open_tmux(session, window_id.as_deref(), window, cx);
                });
                Tab {
                    id,
                    view: TabView::Start(page),
                    _subscription: subscription,
                }
            }
            Opening::Tmux { session, window } => self.tmux_tab(id, &session, window.as_deref(), cx),
        }
    }

    fn index_of(&self, id: usize) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        self.active = ix;
        let handle = tab.focus_handle(cx);
        window.focus(&handle, cx);
        let view = tab.tmux().map(|view| view.read(cx).view().to_string());
        self.sidebar.update(cx, |sidebar, cx| sidebar.set_current(view, cx));
        self.schedule_save(window, cx);
        cx.notify();
    }

    /// Shows a tmux window in a tab of its own: the tab already on it, or else a new one (in
    /// place of a new-tab page). With no window given, a tab already on the session will do,
    /// or else a new one on the window the session is on.
    pub fn open_tmux(&mut self, session: &str, window_id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let active = Some(self.active).filter(|ix| self.tabs[*ix].shows(session, window_id, cx));
        let showing = active.or_else(|| self.tabs.iter().position(|tab| tab.shows(session, window_id, cx)));
        if let Some(ix) = showing {
            self.activate(ix, window, cx);
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let tab = self.tmux_tab(id, session, window_id, cx);
        // The sidebar shows it as open right away rather than at the next look at tmux.
        if let (Some(window_id), Some(view)) = (window_id, tab.tmux()) {
            let view = view.read(cx).view().to_string();
            watcher::get(cx).update(cx, |watcher, cx| watcher.mark_active(&view, window_id, cx));
        }
        match self.tabs.get(self.active) {
            Some(Tab {
                view: TabView::Start(_),
                ..
            }) => {
                let ix = self.active;
                self.tabs[ix] = tab;
                self.activate(ix, window, cx);
            }
            _ => {
                self.tabs.push(tab);
                self.activate(self.tabs.len() - 1, window, cx);
            }
        }
    }

    fn tmux_tab(&mut self, id: usize, session: &str, window_id: Option<&str>, cx: &mut Context<Self>) -> Tab {
        let view = cx.new(|cx| TmuxTab::new(session.to_string(), window_id, cx));
        let subscription = cx.observe(&view, |_, _, cx| cx.notify());
        Tab {
            id,
            view: TabView::Tmux(view),
            _subscription: subscription,
        }
    }

    /// Opens the Cmd-K palette as soon as tmux has been read (at launch it may not have been).
    pub fn go_to_when_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let watcher = watcher::get(cx);
        if watcher.read(cx).machine.is_some() {
            self.go_to_window(&GoToWindow, window, cx);
            return;
        }
        let pending: std::rc::Rc<std::cell::Cell<Option<Subscription>>> = Default::default();
        let slot = pending.clone();
        let subscription = cx.observe_in(&watcher, window, move |shell, watcher, window, cx| {
            if watcher.read(cx).machine.is_some() && slot.take().is_some() {
                shell.go_to_window(&GoToWindow, window, cx);
            }
        });
        pending.set(Some(subscription));
    }

    /// Cmd-K: a palette of every tmux window, needs-you first. Type to filter by session,
    /// window, Claude session or what it waits for; Enter goes there.
    fn go_to_window(&mut self, _: &GoToWindow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(machine) = watcher::get(cx).read(cx).machine.clone() else {
            return;
        };
        let entries = std::rc::Rc::new(switcher::entries(&machine));
        let state = cx.new(|cx| CommandState::new(window, cx));
        let shell = cx.entity().downgrade();
        let palette = state.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let items = entries.iter().map(|entry| {
                let keywords = [entry.claude.clone(), entry.waiting.clone()].into_iter().flatten();
                let row = entry.clone();
                CommandItem::new()
                    .label(format!("{} › {}", entry.session, entry.label))
                    .keywords(keywords)
                    .child(move |_, _| palette_row(&row))
            });
            let (chosen, shell) = (entries.clone(), shell.clone());
            dialog
                .w(px(620.))
                .p_0()
                .close_button(false)
                .child(
                    Command::new(&palette)
                        .bordered(false)
                        .placeholder("Go to a window…")
                        .max_h(px(420.))
                        .group(CommandGroup::new().items(items))
                        .on_confirm(move |index, window, cx| {
                            window.close_dialog(cx);
                            if let Some(entry) = chosen.get(index.row) {
                                let (session, window_id) = (entry.session.clone(), entry.window_id.clone());
                                let _ = shell.update(cx, |shell, cx| shell.open_tmux(&session, Some(&window_id), window, cx));
                            }
                        }),
                )
        });
        state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Shows the next window whose Claude session is waiting on you, after the one on screen.
    fn next_needing_you(&mut self, _: &NextNeedingYou, window: &mut Window, cx: &mut Context<Self>) {
        let Some(machine) = watcher::get(cx).read(cx).machine.clone() else {
            return;
        };
        let waiting: Vec<(String, String)> = machine
            .sessions
            .iter()
            .flat_map(|session| {
                session
                    .windows
                    .iter()
                    .filter(|w| matches!(machine.window_state(w), Some(ClaudeState::NeedsYou(_))))
                    .map(|w| (session.name.clone(), w.id.clone()))
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let on_screen = self.tabs.get(self.active).and_then(|tab| tab.tmux()).and_then(|view| {
            let view = view.read(cx);
            Some((view.session().to_string(), view.window_id()?.to_string()))
        });
        let next = match on_screen.and_then(|current| waiting.iter().position(|w| *w == current)) {
            Some(ix) => waiting[(ix + 1) % waiting.len()].clone(),
            None => waiting[0].clone(),
        };
        self.open_tmux(&next.0, Some(&next.1), window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
        self.schedule_save(window, cx);
        cx.notify();
    }

    /// The sidebar's right edge: drag it to make the sidebar wider or narrower, double-click it
    /// for the usual width.
    fn sidebar_edge(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("sidebar-edge")
            .debug_selector(|| "sidebar-edge".to_string())
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(self.sidebar_width - 3.))
            .w(px(6.))
            .cursor(CursorStyle::ResizeLeftRight)
            .on_drag(DraggedEdge, |_, _, _, cx| cx.new(|_| DraggedEdge))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, event: &MouseDownEvent, window, cx| {
                    if event.click_count == 2 {
                        shell.resize_sidebar(SIDEBAR_WIDTH, window, cx);
                    }
                }),
            )
    }

    fn resize_sidebar(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        let width = width.clamp(*SIDEBAR_WIDTHS.start(), *SIDEBAR_WIDTHS.end()).round();
        if width != self.sidebar_width {
            self.sidebar_width = width;
            self.schedule_save(window, cx);
            cx.notify();
        }
    }

    /// Puts back the active tab and the sidebar as they were last time.
    pub fn restore_view(
        &mut self,
        active: usize,
        sidebar: bool,
        sidebar_width: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_sidebar = sidebar;
        if let Some(width) = sidebar_width {
            self.sidebar_width = width.clamp(*SIDEBAR_WIDTHS.start(), *SIDEBAR_WIDTHS.end());
        }
        self.activate(active.min(self.tabs.len().saturating_sub(1)), window, cx);
    }

    /// The window as it is now, as it would open next time.
    fn window_state(&self, window: &Window, cx: &App) -> SavedWindow {
        let mut tabs = Vec::new();
        let mut active = 0;
        for (ix, tab) in self.tabs.iter().enumerate() {
            let saved = match &tab.view {
                TabView::Start(_) => Some(SavedTab::Start),
                TabView::Tmux(view) => {
                    let view = view.read(cx);
                    Some(SavedTab::Tmux {
                        session: view.session().to_string(),
                        window: view.window_id().map(str::to_string),
                    })
                }
            };
            if let Some(saved) = saved {
                if ix == self.active {
                    active = tabs.len();
                }
                tabs.push(saved);
            }
        }
        let (bounds, state) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (bounds, WindowState::Windowed),
            WindowBounds::Maximized(bounds) => (bounds, WindowState::Maximized),
            WindowBounds::Fullscreen(bounds) => (bounds, WindowState::Fullscreen),
        };
        SavedWindow {
            tabs,
            active,
            sidebar: self.show_sidebar,
            sidebar_width: (self.sidebar_width != SIDEBAR_WIDTH).then_some(self.sidebar_width),
            bounds: Some(SavedBounds {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                state,
            }),
        }
    }

    /// Saves the window's state a moment after it stops changing, and only if it changed.
    fn schedule_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._save = Some(cx.spawn_in(window, async move |shell, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            shell
                .update_in(cx, |shell, window, cx| {
                    let state = shell.window_state(window, cx);
                    if shell.saved.as_ref() != Some(&state) && store::save_window(&state).is_ok() {
                        shell.saved = Some(state);
                    }
                })
                .ok();
        }));
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.make_tab(Opening::Start, window, cx);
        self.tabs.push(tab);
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn close_active(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.remove_tab(ix, window, cx);
        }
    }

    /// Closing the last session leaves a start tab; closing a lone start tab closes the window.
    fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 && matches!(self.tabs[0].view, TabView::Start(_)) {
            window.remove_window();
            return;
        }
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            let tab = self.make_tab(Opening::Start, window, cx);
            self.tabs.push(tab);
        }
        let active = if ix < self.active {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.activate(active, window, cx);
    }

    fn activate_tab(&mut self, action: &ActivateTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(action.0, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate((self.active + 1) % self.tabs.len(), window, cx);
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate((self.active + self.tabs.len() - 1) % self.tabs.len(), window, cx);
    }

    /// Closes every tab but `ix`.
    fn close_others(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(keep) = self.tabs.get(ix).map(|tab| tab.id) else { return };
        while let Some(other) = self.tabs.iter().position(|tab| tab.id != keep) {
            self.close_tab(other, window, cx);
        }
    }

    /// The tabs, at the top of the middle column. Its empty parts move and zoom the window like
    /// any title bar; with the sidebar hidden, it starts after the window buttons.
    fn tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut strip = div().flex().items_center().gap(px(4.)).min_w_0().overflow_hidden();
        for (ix, tab) in self.tabs.iter().enumerate() {
            let active = ix == self.active;
            let id = tab.id;
            let (title, light): (SharedString, Option<u32>) = match &tab.view {
                TabView::Start(_) => ("New Tab".into(), None),
                TabView::Tmux(view) => (
                    view.read(cx).title(cx),
                    view.read(cx).state(cx).map(|state| crate::sidebar::light_color(&state)),
                ),
            };
            let group = SharedString::from(format!("tab-{id}"));
            let tip = title.clone();
            let shell = cx.entity().downgrade();
            strip = strip.child(
                div()
                    .id(("tab", id))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .h(px(28.))
                    .pl(px(10.))
                    .pr(px(5.))
                    .min_w(px(112.))
                    .max_w(px(220.))
                    .flex_shrink(1.)
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .when(active, |tab| tab.bg(rgba(theme::GLASS_SELECTED)).text_color(rgb(theme::TEXT)))
                    .when(!active, |tab| {
                        tab.text_color(rgb(theme::TEXT_MUTED)).cursor_pointer().hover(|style| {
                            style.bg(rgba(theme::GLASS_HOVER)).text_color(rgb(theme::TEXT))
                        })
                    })
                    // A tab isn't the title bar: pressing it doesn't move the window.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |shell, _: &ClickEvent, window, cx| {
                        if let Some(ix) = shell.index_of(id) {
                            shell.activate(ix, window, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |shell, _: &MouseDownEvent, window, cx| {
                            if let Some(ix) = shell.index_of(id) {
                                shell.close_tab(ix, window, cx);
                            }
                        }),
                    )
                    .tooltip(move |window, cx| {
                        Tooltip::new(tip.clone())
                            .action(&ActivateTab(ix), None)
                            .build(window, cx)
                    })
                    .children(light.map(crate::sidebar::light))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(("close-tab", id))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .text_size(px(13.))
                            .text_color(rgb(theme::TEXT_FAINT))
                            .when(!active, |close| close.invisible().group_hover(group, |style| style.visible()))
                            .hover(|style| style.bg(rgba(theme::GLASS_HOVER)).text_color(rgb(theme::TEXT)))
                            .child("×")
                            .on_click(cx.listener(move |shell, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if let Some(ix) = shell.index_of(id) {
                                    shell.close_tab(ix, window, cx);
                                }
                            })),
                    )
                    .context_menu(move |menu, _, _| {
                        let (close, others) = (shell.clone(), shell.clone());
                        menu.item(PopupMenuItem::new("Close Tab").on_click(move |_, window, cx| {
                            let _ = close.update(cx, |shell, cx| {
                                if let Some(ix) = shell.index_of(id) {
                                    shell.close_tab(ix, window, cx);
                                }
                            });
                        }))
                        .item(PopupMenuItem::new("Close Other Tabs").on_click(move |_, window, cx| {
                            let _ = others.update(cx, |shell, cx| {
                                if let Some(ix) = shell.index_of(id) {
                                    shell.close_others(ix, window, cx);
                                }
                            });
                        }))
                    }),
            );
        }
        let new_tab = div()
            .id("new-tab")
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_size(px(16.))
            .text_color(rgb(theme::TEXT_FAINT))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(theme::GLASS_HOVER)).text_color(rgb(theme::TEXT)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .tooltip(|window, cx| Tooltip::new("New Tab").action(&NewTab, None).build(window, cx))
            .child("+")
            .on_click(cx.listener(|shell, _: &ClickEvent, window, cx| shell.new_tab(&NewTab, window, cx)));
        let clear_of_buttons = !self.show_sidebar && !window.is_fullscreen();
        TitleBar::new()
            .h(px(40.))
            .pl(px(if clear_of_buttons { 84. } else { 8. }))
            .child(div().flex().items_center().gap(px(4.)).min_w_0().child(strip).child(new_tab))
    }

    /// The status bar: who needs you (click, or Cmd-J), what's working, the account's usage
    /// limits, and Cmd-K.
    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let machine = watcher::get(cx).read(cx).machine.clone();
        let entries = machine.as_ref().map(switcher::entries).unwrap_or_default();
        let tallies = machine.as_ref().map(|machine| machine.tallies()).unwrap_or_default();
        let attention = match entries.iter().find(|entry| matches!(entry.state, Some(ClaudeState::NeedsYou(_)))) {
            Some(entry) => {
                let who = entry.claude.clone().unwrap_or_else(|| entry.label.clone());
                div()
                    .id("needs-you")
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(6.))
                    .h(px(20.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(theme::GLASS_HOVER)))
                    .child(crate::sidebar::light(theme::ACCENT))
                    .child(div().text_color(rgb(theme::ACCENT)).child(format!("{who} needs you")))
                    .child(
                        div()
                            .text_color(rgb(theme::TEXT_FAINT))
                            .child(format!("{} › {}", entry.session, entry.label)),
                    )
                    .child(div().text_color(rgb(theme::TEXT_FAINT)).child("⌘J"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NextNeedingYou), cx))
                    .into_any_element()
            }
            None if tallies.working > 0 => div()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(6.))
                .child(crate::sidebar::light(theme::WORKING))
                .child(div().text_color(rgb(theme::TEXT_MUTED)).child(format!(
                    "{} working",
                    tallies.working
                )))
                .into_any_element(),
            None => div()
                .px(px(6.))
                .text_color(rgb(theme::TEXT_FAINT))
                .child("Nothing needs you")
                .into_any_element(),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let limit = |label: &str, limit: crate::status::Limit| {
            let pct = limit.used_pct.min(100);
            let fill = match pct {
                90.. => theme::ERR,
                75.. => theme::ACCENT,
                _ => theme::TEXT_MUTED,
            };
            let resets = limit
                .resets_at
                .filter(|at| *at > now)
                .map(|at| crate::status::until(at - now));
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().text_color(rgb(theme::TEXT_FAINT)).child(label.to_string()))
                .child(
                    div().w(px(40.)).h(px(4.)).rounded(px(2.)).bg(rgb(theme::LED_OFF)).child(
                        div()
                            .h_full()
                            .w(px(40. * pct as f32 / 100.))
                            .rounded(px(2.))
                            .bg(rgb(fill)),
                    ),
                )
                .child(div().text_color(rgb(theme::TEXT_MUTED)).child(format!("{pct}%")))
                .children(resets.map(|resets| div().text_color(rgb(theme::TEXT_FAINT)).child(resets)))
        };
        let limits = machine.as_ref().and_then(|machine| machine.limits().cloned());
        let mut right = div().flex().items_center().gap(px(18.));
        if let Some(status) = limits {
            right = right
                .children(status.five_hour.map(|five| limit("5h", five)))
                .children(status.seven_day.map(|seven| limit("7d", seven)));
        }
        right = right.child(
            div()
                .id("go-to")
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .h(px(20.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(theme::GLASS_HOVER)))
                .child(div().text_color(rgb(theme::TEXT_MUTED)).child("Go to"))
                .child(div().text_color(rgb(theme::TEXT_FAINT)).child("⌘K"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(GoToWindow), cx)),
        );
        StatusBar::new()
            .h(px(28.))
            .px(px(10.))
            .text_size(px(11.))
            .left(attention)
            .right(right)
    }
}

/// A row of the Cmd-K palette: the window's light, where it is, its Claude session, and what
/// it waits for.
fn palette_row(entry: &Entry) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_1()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .text_size(px(13.))
        .child(
            div()
                .flex_none()
                .w(px(8.))
                .flex()
                .justify_center()
                .children(entry.state.as_ref().map(|state| crate::sidebar::light(crate::sidebar::light_color(state)))),
        )
        .child(div().flex_none().text_color(rgb(theme::TEXT_MUTED)).child(entry.session.clone()))
        .child(div().flex_none().text_color(rgb(theme::TEXT_FAINT)).child("›"))
        .child(div().flex_1().min_w_0().truncate().text_color(rgb(theme::TEXT)).child(entry.label.clone()))
        .children(entry.claude.clone().map(|name| {
            div()
                .flex_none()
                .max_w(px(180.))
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(theme::TEXT_FAINT))
                .child(name)
        }))
        .children(entry.waiting.clone().map(|what| {
            div()
                .flex_none()
                .max_w(px(140.))
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(theme::ACCENT))
                .child(what)
        }))
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (content, title): (AnyElement, String) = match &self.tabs[self.active].view {
            TabView::Start(page) => (page.clone().into_any_element(), "Signalbox".to_string()),
            TabView::Tmux(view) => (
                view.clone().into_any_element(),
                format!("{} — Signalbox", view.read(cx).title(cx)),
            ),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        // A finished turn you're already looking at gets no notification.
        if window.is_window_active() {
            let on_screen = self.tabs[self.active].tmux().and_then(|view| {
                let view = view.read(cx);
                Some((view.session().to_string(), view.window_id()?.to_string()))
            });
            crate::alerts::set_on_screen(on_screen);
        }
        let media = self.tabs[self.active].tmux().cloned().and_then(|tab| {
            let (shown, media) = {
                let view = tab.read(cx);
                (view.panel_shown(cx), view.media(cx))
            };
            shown.then(|| crate::media_panel::panel(&tab, &media))
        });
        div()
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active))
            .on_action(cx.listener(Self::activate_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::go_to_window))
            .on_action(cx.listener(Self::next_needing_you))
            .relative()
            .size_full()
            .flex()
            .text_color(rgb(theme::TEXT))
            .on_drag_move(cx.listener(|shell, event: &DragMoveEvent<DraggedEdge>, window, cx| {
                let width = event.event.position.x - event.bounds.origin.x;
                shell.resize_sidebar(f32::from(width), window, cx);
            }))
            // Three columns: the sidebar and the media on glass (the window's background is
            // blurred), and between them the tabs over an opaque terminal.
            .children(self.show_sidebar.then(|| {
                div()
                    .w(px(self.sidebar_width))
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(theme::BORDER))
                    // Where the window buttons sit.
                    .child(TitleBar::new().h(px(40.)).border_b_0())
                    .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.tab_bar(window, cx))
                    .child(
                        div()
                            .debug_selector(|| "content".to_string())
                            .flex_1()
                            .min_h_0()
                            .bg(rgb(theme::BG))
                            .child(content),
                    )
                    .child(self.status_bar(cx)),
            )
            .children(media)
            // Last, so it's grabbable over the content's edge too.
            .children(self.show_sidebar.then(|| self.sidebar_edge(cx)))
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs
            .get(self.active)
            .map(|tab| tab.focus_handle(cx))
            .unwrap_or_else(|| self.focus_handle.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, Pixels, Point, TestAppContext, VisualTestContext, point};

    /// A window with a new tab, which opens nothing: no tmux.
    fn window(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::watcher::init_idle(cx);
        });
        let (shell, cx) = cx.add_window_view(|window, cx| Shell::new(vec![Opening::Start], window, cx));
        cx.run_until_parked();
        (shell, cx)
    }

    fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        let none = Modifiers::none();
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(from + point(px(10.), px(0.)), MouseButton::Left, none);
        cx.simulate_mouse_move(to, MouseButton::Left, none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
    }

    #[gpui::test]
    fn the_sidebar_is_as_wide_as_its_edge_is_dragged(cx: &mut TestAppContext) {
        let (shell, cx) = window(cx);
        let edge = cx.debug_bounds("sidebar-edge").expect("the sidebar's edge is drawn");
        let y = edge.center().y;
        drag(cx, edge.center(), point(px(330.), y));
        assert_eq!(shell.read_with(cx, |shell, _| shell.sidebar_width), 330.);

        // No wider than its widest, and the edge follows.
        let edge = cx.debug_bounds("sidebar-edge").unwrap();
        drag(cx, edge.center(), point(px(900.), y));
        assert_eq!(shell.read_with(cx, |shell, _| shell.sidebar_width), *SIDEBAR_WIDTHS.end());

        // A double-click puts the usual width back.
        let edge = cx.debug_bounds("sidebar-edge").unwrap();
        cx.simulate_event(MouseDownEvent {
            position: edge.center(),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        assert_eq!(shell.read_with(cx, |shell, _| shell.sidebar_width), SIDEBAR_WIDTH);
    }
}
