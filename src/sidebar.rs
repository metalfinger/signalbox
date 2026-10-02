//! The left column, a source list on glass: every tmux session and its windows, with a light
//! for the Claude session in each (steel working, champagne needs you, dim idle) and how full
//! its context is. What it shows comes from the app-wide [`Watcher`].

use std::collections::HashSet;

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FontWeight, SharedString, Subscription,
    WeakEntity, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};

use crate::live::ClaudeState;
use crate::theme;
use crate::tmux::{TmuxSession, TmuxWindow};
use crate::ui;
use crate::watcher::{Machine, Restoring, Tallies, Watcher};

pub enum SidebarEvent {
    /// Show this session, and this window in it when given.
    Open { session: String, window_id: Option<String> },
}

pub struct Sidebar {
    watcher: Entity<Watcher>,
    collapsed: HashSet<String>,
    /// The active tab's view of its session: the window it shows is highlighted here.
    current: Option<String>,
    _watch: Subscription,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

/// A window's light: its Claude session's state, or none for a plain shell.
pub fn light_color(state: &ClaudeState) -> u32 {
    match state {
        ClaudeState::NeedsYou(_) => theme::ACCENT,
        ClaudeState::Working => theme::WORKING,
        ClaudeState::Shell | ClaudeState::Idle => theme::LED_OFF,
    }
}

pub fn light(color: u32) -> impl IntoElement {
    div().flex_none().size(px(7.)).rounded_full().bg(rgb(color))
}

/// "● 1  ● 3": how many need you and how many are working, when any are.
pub fn tallies(tallies: Tallies) -> impl IntoElement {
    let count = |n: usize, color: u32| {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .text_color(rgb(color))
            .child(light(color))
            .child(n.to_string())
    };
    div()
        .flex()
        .items_center()
        .gap_3()
        .text_size(px(11.))
        .children((tallies.needs_you > 0).then(|| count(tallies.needs_you, theme::ACCENT)))
        .children((tallies.working > 0).then(|| count(tallies.working, theme::WORKING)))
}

impl Sidebar {
    pub fn new(watcher: Entity<Watcher>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&watcher, |_, _, cx| cx.notify());
        Self {
            watcher,
            collapsed: HashSet::new(),
            current: None,
            _watch: watch,
        }
    }

    pub fn set_current(&mut self, view: Option<String>, cx: &mut Context<Self>) {
        if self.current != view {
            self.current = view;
            cx.notify();
        }
    }

    fn toggle(&mut self, session: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(session) {
            self.collapsed.insert(session.to_string());
        }
        cx.notify();
    }

    fn session_row(&self, session: &TmuxSession, machine: &Machine, cx: &mut Context<Self>) -> AnyElement {
        let name = session.name.clone();
        let collapsed = self.collapsed.contains(&name);
        let mut counts = Tallies::default();
        for window in &session.windows {
            match machine.window_state(window) {
                Some(ClaudeState::NeedsYou(_)) => counts.needs_you += 1,
                Some(ClaudeState::Working) => counts.working += 1,
                _ => {}
            }
        }
        let quiet = counts.needs_you + counts.working == 0;
        let open = name.clone();
        let fold = name.clone();
        div()
            .id(SharedString::from(format!("session-{name}")))
            .flex()
            .items_center()
            .gap_2()
            .mx(px(6.))
            .px(px(8.))
            .h(px(28.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(theme::GLASS_HOVER)))
            .child(
                div()
                    .id(SharedString::from(format!("fold-{name}")))
                    .flex_none()
                    .w(px(12.))
                    .text_size(px(10.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .hover(|style| style.text_color(rgb(theme::TEXT)))
                    .child(if collapsed { "▸" } else { "▾" })
                    .on_click(cx.listener(move |sidebar, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        sidebar.toggle(&fold, cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::TEXT))
                    .child(name.clone()),
            )
            .child(if quiet {
                div()
                    .text_size(px(11.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child(session.windows.len().to_string())
                    .into_any_element()
            } else {
                tallies(counts).into_any_element()
            })
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.emit(SidebarEvent::Open {
                    session: open.clone(),
                    window_id: None,
                })
            }))
            .into_any_element()
    }

    fn window_row(&self, session: &str, window: &TmuxWindow, machine: &Machine, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.current.as_ref().and_then(|view| machine.views.get(view)) == Some(&window.id);
        let claude = machine.window_claude(window);
        let state = claude.map(|claude| claude.state.clone());
        let needs_you = matches!(state, Some(ClaudeState::NeedsYou(_)));
        let working = state == Some(ClaudeState::Working);
        // How full the context is, when the window's Claude has reported it.
        let context = claude
            .and_then(|session| session.status.as_ref())
            .map(|status| status.context_pct);
        let right = if needs_you {
            div()
                .text_size(px(11.))
                .text_color(rgb(theme::ACCENT))
                .child("Waiting")
                .into_any_element()
        } else {
            div()
                .text_size(px(11.))
                .text_color(rgb(if context.is_some_and(|pct| pct >= 90) {
                    theme::ERR
                } else {
                    theme::TEXT_FAINT
                }))
                .children(context.map(|pct| format!("{pct}%")))
                .into_any_element()
        };
        let (open_session, open_window) = (session.to_string(), window.id.clone());
        let sidebar = cx.entity().downgrade();
        let (menu_session, menu_window, menu_name) = (session.to_string(), window.id.clone(), window.name.clone());
        div()
            .id(SharedString::from(format!("window-{}", window.id)))
            .flex()
            .items_center()
            .gap_2()
            .mx(px(6.))
            .pl(px(28.))
            .pr(px(10.))
            .h(px(28.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |row| row.bg(rgba(theme::GLASS_SELECTED)))
            .when(!selected, |row| row.hover(|style| style.bg(rgba(theme::GLASS_HOVER))))
            .child(
                div()
                    .flex_none()
                    .w(px(8.))
                    .flex()
                    .justify_center()
                    .children(state.as_ref().map(|state| light(light_color(state)))),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .text_size(px(11.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child(window.index.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(rgb(if needs_you {
                        theme::ACCENT
                    } else if selected || working {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    }))
                    .child(window.name.clone()),
            )
            .child(right)
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.emit(SidebarEvent::Open {
                    session: open_session.clone(),
                    window_id: Some(open_window.clone()),
                })
            }))
            .context_menu(move |menu, _, _| {
                let (sidebar, session, window_id, name) =
                    (sidebar.clone(), menu_session.clone(), menu_window.clone(), menu_name.clone());
                menu.item(PopupMenuItem::new("Open").on_click(move |_, _, cx| {
                    open_from_menu(&sidebar, &session, Some(&window_id), cx);
                }))
                .item(PopupMenuItem::new("Copy Window Name").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
                }))
            })
            .into_any_element()
    }

    fn restoring_card(&self, restoring: &Restoring, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = div()
            .mx(px(12.))
            .mb_3()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .rounded(px(8.))
            .bg(rgba(theme::GLASS_HOVER))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .text_size(px(12.))
            .text_color(rgb(theme::TEXT_MUTED));
        match restoring {
            Restoring::Idle => None,
            Restoring::Running(_) => Some(card.child("Rebuilding your workspace…").into_any_element()),
            Restoring::Done { title, lines } => Some(
                card.child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::TEXT))
                        .child(title.clone()),
                )
                .children(lines.iter().map(|line| div().child(line.clone())))
                .child(
                    div().pt_1().child(
                        ui::button("restored-ok", "OK", ui::ButtonKind::Quiet).on_click(cx.listener(
                            |sidebar, _: &ClickEvent, _, cx| {
                                sidebar.watcher.update(cx, |watcher, cx| watcher.clear_report(cx))
                            },
                        )),
                    ),
                )
                .into_any_element(),
            ),
        }
    }
}

fn open_from_menu(sidebar: &WeakEntity<Sidebar>, session: &str, window_id: Option<&str>, cx: &mut gpui::App) {
    let _ = sidebar.update(cx, |_, cx| {
        cx.emit(SidebarEvent::Open {
            session: session.to_string(),
            window_id: window_id.map(str::to_string),
        })
    });
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let watcher = self.watcher.read(cx);
        let machine = watcher.machine.clone();
        let tmux_error = watcher.tmux_error.clone();
        let restoring = match &watcher.restoring {
            Restoring::Idle => Restoring::Idle,
            Restoring::Running(sessions) => Restoring::Running(sessions.clone()),
            Restoring::Done { title, lines } => Restoring::Done {
                title: title.clone(),
                lines: lines.clone(),
            },
        };

        let mut list = div().flex().flex_col().gap(px(10.)).pb_4();
        // What's below is the last good read: say so rather than show it as current.
        if let Some(error) = &tmux_error {
            list = list.child(
                div()
                    .mx(px(12.))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded(px(8.))
                    .bg(rgba(theme::GLASS_HOVER))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .text_size(px(12.))
                    .child(div().text_color(rgb(theme::ACCENT)).child("Can't read tmux"))
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child(error.clone())),
            );
        }
        if let Some(machine) = &machine {
            if machine.sessions.is_empty() && tmux_error.is_none() {
                list = list.child(
                    div()
                        .mx(px(16.))
                        .mt_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.))
                        .child(div().text_color(rgb(theme::TEXT_MUTED)).child("No tmux sessions"))
                        .child(
                            div()
                                .text_color(rgb(theme::TEXT_FAINT))
                                .child("Start one in any terminal with tmux new -s name."),
                        ),
                );
            }
            for session in &machine.sessions {
                let mut group = div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(self.session_row(session, machine, cx));
                if !self.collapsed.contains(&session.name) {
                    for window in &session.windows {
                        group = group.child(self.window_row(&session.name, window, machine, cx));
                    }
                }
                list = list.child(group);
            }
        }
        let header_tallies = machine.as_ref().map(|machine| tallies(machine.tallies()));
        let restoring = self.restoring_card(&restoring, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(8.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::TEXT_FAINT))
                            .child("Sessions"),
                    )
                    .children(header_tallies),
            )
            .children(restoring)
            .child(div().id("sidebar").flex_1().min_h_0().overflow_y_scroll().child(list))
    }
}
