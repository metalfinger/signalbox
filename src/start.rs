use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString, Subscription,
    Window, div, prelude::*, px, rgb,
};

use crate::live::ClaudeState;
use crate::switcher::{self, Entry};
use crate::theme;
use crate::watcher::{self, Machine, Watcher};

pub enum StartEvent {
    /// Show this tmux session, and this window in it when given.
    OpenTmux {
        session: String,
        window_id: Option<String>,
    },
}

/// What a new tab shows: what needs you, what's working, then every tmux session.
pub struct StartPage {
    focus_handle: FocusHandle,
    watcher: Entity<Watcher>,
    _watch: Subscription,
}

impl EventEmitter<StartEvent> for StartPage {}

impl StartPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let watcher = watcher::get(cx);
        let watch = cx.observe(&watcher, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            watcher,
            _watch: watch,
        }
    }
}

/// A grouped list, as macOS draws one: a rounded panel with hairline rows.
fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .bg(rgb(theme::PANEL))
        .overflow_hidden()
}

fn section(label: &str, color: u32) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color))
        .child(label.to_string())
}

/// The app's mark in miniature: a signal head, steel above champagne.
fn signal_glyph() -> impl IntoElement {
    div()
        .flex_none()
        .w(px(12.))
        .h(px(22.))
        .rounded(px(6.))
        .bg(rgb(0x0a0a0b))
        .border_1()
        .border_color(rgb(theme::LED_OFF))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.))
        .child(div().size(px(5.)).rounded_full().bg(rgb(theme::WORKING)))
        .child(div().size(px(5.)).rounded_full().bg(rgb(theme::ACCENT)))
}

/// What needs you and what's working, then every tmux session: each a click away.
fn now(machine: &Machine, cx: &mut Context<StartPage>) -> impl IntoElement + use<> {
    let entries = switcher::entries(machine);
    let row = |entry: &Entry, ix: usize, color: u32, cx: &mut Context<StartPage>| {
        let (session, window_id) = (entry.session.clone(), entry.window_id.clone());
        div()
            .id(SharedString::from(format!("now-{}", entry.window_id)))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .h(px(40.))
            .when(ix > 0, |row| row.border_t_1().border_color(rgb(theme::BORDER)))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::SURFACE_HOVER)))
            .child(crate::sidebar::light(color))
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(entry.session.clone()),
            )
            .child(div().text_size(px(13.)).text_color(rgb(theme::TEXT_FAINT)).child("›"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(rgb(theme::TEXT))
                    .child(entry.label.clone()),
            )
            .children(entry.claude.clone().map(|name| {
                div()
                    .flex_none()
                    .max_w(px(200.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child(name)
            }))
            .children(entry.waiting.clone().map(|what| {
                div()
                    .flex_none()
                    .max_w(px(160.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(theme::ACCENT))
                    .child(what)
            }))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.emit(StartEvent::OpenTmux {
                    session: session.clone(),
                    window_id: Some(window_id.clone()),
                })
            }))
    };
    let mut column = div().flex().flex_col().gap(px(28.));
    for (label, wanted, color) in [
        ("Needs you", true, theme::ACCENT),
        ("Working", false, theme::WORKING),
    ] {
        let picked: Vec<&Entry> = entries
            .iter()
            .filter(|entry| match entry.state {
                Some(ClaudeState::NeedsYou(_)) => wanted,
                Some(ClaudeState::Working) => !wanted,
                _ => false,
            })
            .collect();
        if picked.is_empty() {
            continue;
        }
        let mut list = card();
        for (ix, entry) in picked.into_iter().enumerate() {
            list = list.child(row(entry, ix, color, cx));
        }
        column = column.child(div().flex().flex_col().gap(px(10.)).child(section(label, color)).child(list));
    }
    if machine.sessions.is_empty() {
        column = column.child(
            card().child(
                div()
                    .px(px(16.))
                    .py(px(14.))
                    .text_size(px(13.))
                    .text_color(rgb(theme::TEXT_FAINT))
                    .child("No tmux sessions yet. Start one in any terminal with tmux new -s name."),
            ),
        );
    } else {
        let mut tiles = div().grid().grid_cols(3).gap(px(10.));
        for session in &machine.sessions {
            let name = session.name.clone();
            let mut counts = crate::watcher::Tallies::default();
            for window in &session.windows {
                match machine.window_state(window) {
                    Some(ClaudeState::NeedsYou(_)) => counts.needs_you += 1,
                    Some(ClaudeState::Working) => counts.working += 1,
                    _ => {}
                }
            }
            let windows = session.windows.len();
            tiles = tiles.child(
                div()
                    .id(SharedString::from(format!("tile-{name}")))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .bg(rgb(theme::PANEL))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::SURFACE_HOVER)).border_color(rgb(theme::LED_OFF)))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::TEXT))
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme::TEXT_FAINT))
                                    .child(format!("{windows} window{}", if windows == 1 { "" } else { "s" })),
                            )
                            .child(crate::sidebar::tallies(counts)),
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(StartEvent::OpenTmux {
                            session: name.clone(),
                            window_id: None,
                        })
                    })),
            );
        }
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(section("Sessions", theme::TEXT_FAINT))
                .child(tiles),
        );
    }
    column
}

impl Render for StartPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let machine = self.watcher.read(cx).machine.clone();
        let summary = machine.as_ref().map(|machine| {
            let tallies = machine.tallies();
            let mut parts = Vec::new();
            if tallies.needs_you > 0 {
                parts.push(format!(
                    "{} need{} you",
                    tallies.needs_you,
                    if tallies.needs_you == 1 { "s" } else { "" }
                ));
            }
            if tallies.working > 0 {
                parts.push(format!("{} working", tallies.working));
            }
            if parts.is_empty() {
                parts.push("Nothing needs you".to_string());
            }
            let sessions = machine.sessions.len();
            parts.push(format!("{sessions} tmux session{}", if sessions == 1 { "" } else { "s" }));
            parts.join(" · ")
        });
        let now = machine.map(|machine| now(&machine, cx));
        div()
            .id("start")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(px(32.))
            .pt(px(64.))
            .pb(px(48.))
            .bg(rgb(theme::BG))
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .w_full()
                    .max_w(px(680.))
                    .flex()
                    .flex_col()
                    .gap(px(36.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(signal_glyph())
                                    .child(
                                        div()
                                            .font_family(theme::WORDMARK)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(px(26.))
                                            .text_color(rgb(theme::TEXT))
                                            .child("signalbox"),
                                    ),
                            )
                            .children(summary.map(|summary| {
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .child(summary)
                            })),
                    )
                    .children(now),
            )
    }
}

impl Focusable for StartPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
