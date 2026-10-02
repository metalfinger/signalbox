//! A terminal inside the app: a program in a pseudo-terminal (for now a tmux client), emulated by
//! alacritty's terminal library and drawn with gpui.

mod colors;
mod element;
pub mod keys;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use anyhow::Result;
use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle as PointerStyle, EntityInputHandler,
    ExternalPaths, Global, ModifiersChangedEvent, UTF16Selection, EventEmitter, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    Size, Task, Window, actions, div, prelude::*, px, rgb,
};

use crate::theme;
use element::TerminalElement;
use keys::MouseAction;

actions!(terminal, [Copy, Paste, Tab, BackTab, Bigger, Smaller, ActualSize]);

const CONTEXT: &str = "Terminal";
/// Ghostty's `font-size = 14`.
pub const DEFAULT_FONT_SIZE: f32 = 14.;
const FONT_SIZES: std::ops::RangeInclusive<f32> = 9.0..=32.0;
/// How long the cursor stays on, then off, while it blinks; and stays on after typing.
const BLINK: Duration = Duration::from_millis(600);
/// Alacritty's link pattern: the schemes worth opening, up to a character that ends a link.
const LINK_PATTERN: &str = "(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file:|git://|ssh:|ftp://)[^\\x00-\\x1F\\x7F-\\x9F<>\"\\s{-}\\^⟨⟩`]+";

/// The terminals' font size, shared by every terminal (Cmd+/Cmd-/Cmd-0) and remembered.
pub struct TerminalFont(pub f32);

impl Global for TerminalFont {}

pub fn init(cx: &mut App) {
    let size = crate::store::load_settings().font_size.unwrap_or(DEFAULT_FONT_SIZE);
    cx.set_global(TerminalFont(size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end())));
    cx.bind_keys([
        // The kit binds Tab to moving focus for the whole window; the program needs it.
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", BackTab, Some(CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        KeyBinding::new("cmd-=", Bigger, None),
        KeyBinding::new("cmd-+", Bigger, None),
        KeyBinding::new("cmd--", Smaller, None),
        KeyBinding::new("cmd-0", ActualSize, None),
    ]);
    cx.on_action(|_: &Bigger, cx| set_font_size(cx.global::<TerminalFont>().0 + 1., cx));
    cx.on_action(|_: &Smaller, cx| set_font_size(cx.global::<TerminalFont>().0 - 1., cx));
    cx.on_action(|_: &ActualSize, cx| set_font_size(DEFAULT_FONT_SIZE, cx));
}

fn set_font_size(size: f32, cx: &mut App) {
    let size = size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
    if cx.global::<TerminalFont>().0 == size {
        return;
    }
    cx.set_global(TerminalFont(size));
    let mut settings = crate::store::load_settings();
    settings.font_size = (size != DEFAULT_FONT_SIZE).then_some(size);
    if let Err(error) = crate::store::save_settings(&settings) {
        eprintln!("[terminal] couldn't save the font size: {error}");
    }
    cx.refresh_windows();
}

/// A link under the pointer while Cmd is held; Cmd-click opens it.
#[derive(Clone, Debug, PartialEq)]
pub struct HoveredLink {
    pub start: GridPoint,
    pub end: GridPoint,
    pub uri: String,
}

/// Forwards the emulator's events to the view.
#[derive(Clone)]
struct Listener(async_channel::Sender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Cell size and where the grid sits, as last laid out.
#[derive(Clone, Copy, Debug, Default)]
pub struct Layout {
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
}

struct Pty {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
}

impl Pty {
    fn spawn(program: &str, args: &[String], size: GridSize) -> Result<(Self, async_channel::Receiver<TermEvent>)> {
        let (tx, rx) = async_channel::unbounded();
        let listener = Listener(tx);
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "Signalbox".to_string());
        env.insert(
            "PATH".to_string(),
            crate::sys::search_path().to_string_lossy().to_string(),
        );
        if std::env::var_os("LANG").is_none() {
            env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(program.to_string(), args.to_vec())),
            working_directory: std::env::var_os("HOME").map(Into::into),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, window_size(size, Size::default()), 0)?;
        let config = Config {
            // Ghostty's `cursor-style = bar` and `cursor-style-blink = true`; programs may change it.
            default_cursor_style: CursorStyle {
                shape: CursorShape::Beam,
                blinking: true,
            },
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();
        Ok((Self { term, sender }, rx))
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

/// Writes a pasted image to a file a program can open, in the temporary folder macOS clears.
fn save_pasted_image(image: &gpui::Image) -> Option<PathBuf> {
    let extension = image.format.mime_type().strip_prefix("image/")?.split('+').next()?;
    let dir = std::env::temp_dir().join("signalbox-pasted");
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!("pasted-{stamp}.{extension}"));
    std::fs::write(&path, &image.bytes).ok()?;
    Some(path)
}

/// The link at the cell `col`, `row` on screen: a program's own hyperlink (OSC 8) first, else a
/// URL in the text, without what prose adds after it.
fn link_in<T>(term: &Term<T>, col: usize, row: usize, regex: &mut RegexSearch) -> Option<HoveredLink> {
    let offset = term.grid().display_offset() as i32;
    let point = GridPoint::new(Line(row as i32 - offset), Column(col));
    if let Some(link) = term.grid()[point].hyperlink() {
        let same = |p: GridPoint| term.grid()[p].hyperlink().as_ref() == Some(&link);
        let (mut start, mut end) = (point, point);
        while start.column.0 > 0 && same(GridPoint::new(start.line, start.column - 1)) {
            start.column -= 1;
        }
        while end.column.0 + 1 < term.columns() && same(GridPoint::new(end.line, end.column + 1)) {
            end.column += 1;
        }
        return Some(HoveredLink {
            start,
            end,
            uri: link.uri().to_string(),
        });
    }
    let (from, to) = (term.line_search_left(point), term.line_search_right(point));
    let found = RegexIter::new(from, to, Direction::Right, term, regex).find(|m| m.contains(&point))?;
    let text = term.bounds_to_string(*found.start(), *found.end());
    let uri = keys::trim_link(&text);
    let cut = text.chars().count() - uri.chars().count();
    let end = found.end().sub(term, Boundary::Grid, cut);
    (point <= end).then(|| HoveredLink {
        start: *found.start(),
        end,
        uri: uri.to_string(),
    })
}

fn window_size(size: GridSize, cell: Size<Pixels>) -> WindowSize {
    WindowSize {
        num_lines: size.lines as u16,
        num_cols: size.columns as u16,
        cell_width: f32::from(cell.width) as u16,
        cell_height: f32::from(cell.height) as u16,
    }
}

pub enum TerminalEvent {
    /// The program ended.
    Exited,
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    pty: Option<Pty>,
    size: GridSize,
    layout: Layout,
    pressed: Option<u8>,
    /// A drag is selecting text (the terminal's own selection, not the program's).
    selecting: bool,
    last_cell: Option<(usize, usize)>,
    wheel: f32,
    /// Where the pointer is over the terminal, for links found when Cmd goes down.
    pointer: Option<Point<Pixels>>,
    hovered_link: Option<HoveredLink>,
    /// Built on first use: compiling the pattern takes a moment.
    link_regex: Option<RegexSearch>,
    /// Whether a blinking cursor is in its on phase.
    cursor_on: bool,
    /// Set while drawing: the cursor blinks only in the focused terminal of the active window.
    cursor_focused: bool,
    cursor_blinks: bool,
    last_input: Instant,
    /// Text an input method is composing, drawn at the cursor until it's committed.
    marked: Option<String>,
    /// What was typed since the last key that wasn't typing, so macOS can replace it (an
    /// accent picked from the press-and-hold menu replaces the letter just typed).
    recent: String,
    _pump: Option<Task<()>>,
    _blink: Task<()>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    /// A terminal with nothing running in it yet: `start` runs a program.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                if this.update(cx, |view, cx| view.blink(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            pty: None,
            size: GridSize {
                columns: 100,
                lines: 30,
            },
            layout: Layout::default(),
            pressed: None,
            selecting: false,
            last_cell: None,
            wheel: 0.,
            pointer: None,
            hovered_link: None,
            link_regex: None,
            cursor_on: true,
            cursor_focused: false,
            cursor_blinks: false,
            last_input: Instant::now(),
            marked: None,
            recent: String::new(),
            _pump: None,
            _blink: blink,
        }
    }

    /// One step of the cursor's blink. Only the focused terminal blinks, and never while typing,
    /// so a window of idle terminals draws nothing for it.
    fn blink(&mut self, cx: &mut Context<Self>) {
        if !(self.cursor_focused && self.cursor_blinks) {
            if !self.cursor_on {
                self.cursor_on = true;
                cx.notify();
            }
            return;
        }
        if self.last_input.elapsed() < BLINK {
            return;
        }
        self.cursor_on = !self.cursor_on;
        cx.notify();
    }

    /// Typing shows the cursor and keeps it on until typing stops.
    fn typed(&mut self) {
        self.cursor_on = true;
        self.last_input = Instant::now();
    }

    pub fn hovered_link(&self) -> Option<&HoveredLink> {
        self.hovered_link.as_ref()
    }

    fn link_at(&mut self, position: Point<Pixels>) -> Option<HoveredLink> {
        let (col, row) = self.cell_at(position);
        let Self { pty, link_regex, .. } = self;
        let term = pty.as_ref()?.term.lock();
        let regex = link_regex.get_or_insert_with(|| RegexSearch::new(LINK_PATTERN).expect("the link pattern compiles"));
        link_in(&term, col, row, regex)
    }

    /// Shows the link under the pointer while Cmd is held.
    fn update_link(&mut self, command: bool, cx: &mut Context<Self>) {
        let link = match (command, self.pointer) {
            (true, Some(position)) => self.link_at(position),
            _ => None,
        };
        if self.hovered_link != link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn modifiers_changed(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.update_link(event.modifiers.platform, cx);
    }

    /// Runs `program` in the terminal on a fresh screen. It emits `Exited` when the program ends.
    pub fn start(&mut self, program: &str, args: &[String], cx: &mut Context<Self>) -> Result<()> {
        let (pty, events) = Pty::spawn(program, args, self.size)?;
        self.pty = Some(pty);
        self._pump = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let mut batch = vec![event];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |view, cx| view.handle_events(batch, cx)).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
        Ok(())
    }

    fn handle_events(&mut self, batch: Vec<TermEvent>, cx: &mut Context<Self>) {
        for event in batch {
            match event {
                TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
                TermEvent::ColorRequest(index, format) => {
                    let reply = self
                        .pty
                        .as_ref()
                        .map(|pty| format(colors::for_request(index, pty.term.lock().colors())));
                    if let Some(reply) = reply {
                        self.write(reply.into_bytes());
                    }
                }
                TermEvent::TextAreaSizeRequest(format) => {
                    let reply = format(window_size(self.size, self.layout.cell));
                    self.write(reply.into_bytes());
                }
                TermEvent::ChildExit(_) | TermEvent::Exit => self.program_ended(cx),
                _ => {}
            }
        }
        // New output can move or replace the link under the pointer.
        if self.hovered_link.is_some() {
            self.update_link(true, cx);
        }
        cx.notify();
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(pty) = &self.pty {
            pty.write(bytes);
        }
    }

    fn mode(&self) -> TermMode {
        self.pty
            .as_ref()
            .map(|pty| *pty.term.lock().mode())
            .unwrap_or(TermMode::empty())
    }

    /// Called from layout: fit the grid to the space, and tell the program when it changes.
    /// Called while drawing: whether this terminal's cursor may blink right now.
    fn set_cursor_state(&mut self, focused: bool, blinks: bool) {
        self.cursor_focused = focused;
        self.cursor_blinks = blinks;
    }

    pub fn cursor_on(&self) -> bool {
        self.cursor_on
    }

    fn fit(&mut self, size: GridSize, layout: Layout) {
        self.layout = layout;
        if size == self.size || size.columns == 0 || size.lines == 0 {
            return;
        }
        self.size = size;
        if let Some(pty) = &self.pty {
            pty.term.lock().resize(size);
            let _ = pty.sender.send(Msg::Resize(window_size(size, layout.cell)));
        }
    }

    /// The program ended (the terminal can say so more than once): the pty goes, and the tab
    /// hears it once.
    fn program_ended(&mut self, cx: &mut Context<Self>) {
        if self.pty.take().is_some() {
            cx.emit(TerminalEvent::Exited);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Typing reaches the program through `replace_text_in_range` instead.
        if keys::is_text_input(&event.keystroke) {
            return;
        }
        if let Some(bytes) = keys::key_bytes(&event.keystroke, self.mode()) {
            self.clear_selection();
            self.recent.clear();
            self.typed();
            self.scroll_to_bottom();
            self.write(bytes);
            cx.stop_propagation();
        }
    }

    /// Cmd+V. Text goes in as typed; an image goes in as its file, the same as an image dropped
    /// on the terminal (Claude Code attaches it).
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        if let Some(text) = item.text() {
            self.paste_text(&text);
        } else if let Some(path) = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => save_pasted_image(image),
            _ => None,
        }) {
            self.paste_text(&keys::escape_path(&path));
        }
    }

    /// Files dropped from Finder: their paths, escaped, as if typed.
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let text: Vec<String> = paths.paths().iter().map(|path| keys::escape_path(path)).collect();
        self.paste_text(&text.join(" "));
        window.focus(&self.focus_handle, cx);
    }

    /// Typed text, as the program reads typing.
    fn send_text(&mut self, text: &str) {
        self.clear_selection();
        self.typed();
        self.scroll_to_bottom();
        self.recent.push_str(text);
        self.write(text.as_bytes().to_vec());
    }

    /// The screen's text, a line per row, without the spacers after wide characters.
    #[cfg(test)]
    fn screen_text(&self) -> String {
        use alacritty_terminal::term::cell::Flags;
        let Some(pty) = &self.pty else { return String::new() };
        let term = pty.term.lock();
        (0..term.screen_lines())
            .map(|line| {
                let row = &term.grid()[Line(line as i32)];
                (0..term.columns())
                    .map(|col| &row[Column(col)])
                    .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                    .map(|cell| cell.c)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn marked_text(&self) -> Option<&str> {
        self.marked.as_deref()
    }

    /// Where the cursor's cell is on screen, for an input method's candidates.
    fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let term = self.pty.as_ref()?.term.lock();
        let content = term.renderable_content();
        let row = content.cursor.point.line.0 + content.display_offset as i32;
        let Layout { origin, cell } = self.layout;
        (row >= 0).then(|| {
            Bounds::new(
                gpui::point(
                    origin.x + cell.width * content.cursor.point.column.0 as f32,
                    origin.y + cell.height * row as f32,
                ),
                cell,
            )
        })
    }

    fn paste_text(&mut self, text: &str) {
        self.clear_selection();
        self.typed();
        self.scroll_to_bottom();
        self.write(keys::paste_bytes(text, self.mode()));
    }

    fn scroll_to_bottom(&self) {
        if let Some(pty) = &self.pty {
            pty.term.lock().scroll_display(Scroll::Bottom);
        }
    }

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let Layout { origin, cell } = self.layout;
        if cell.width <= px(0.) || cell.height <= px(0.) {
            return (0, 0);
        }
        let col = (f32::from(position.x - origin.x) / f32::from(cell.width)).max(0.) as usize;
        let row = (f32::from(position.y - origin.y) / f32::from(cell.height)).max(0.) as usize;
        (
            col.min(self.size.columns.saturating_sub(1)),
            row.min(self.size.lines.saturating_sub(1)),
        )
    }

    /// The grid point under `position`, and which half of its cell.
    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let (col, row) = self.cell_at(position);
        let offset = self
            .pty
            .as_ref()
            .map_or(0, |pty| pty.term.lock().grid().display_offset() as i32);
        let Layout { origin, cell } = self.layout;
        let into_cell = f32::from(position.x - origin.x) - f32::from(cell.width) * col as f32;
        let side = if into_cell < f32::from(cell.width) / 2. {
            Side::Left
        } else {
            Side::Right
        };
        (GridPoint::new(Line(row as i32 - offset), Column(col)), side)
    }

    /// One click selects from a point, two a word, three a line.
    fn start_selection(&mut self, position: Point<Pixels>, clicks: usize) {
        let (point, side) = self.grid_point(position);
        let kind = match clicks {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        if let Some(pty) = &self.pty {
            pty.term.lock().selection = Some(Selection::new(kind, point, side));
        }
        self.selecting = true;
    }

    /// Whether there was a selection to clear.
    fn clear_selection(&mut self) -> bool {
        self.pty
            .as_ref()
            .is_some_and(|pty| pty.term.lock().selection.take().is_some())
    }

    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.send_key(b"\t", cx);
    }

    fn back_tab(&mut self, _: &BackTab, _: &mut Window, cx: &mut Context<Self>) {
        self.send_key(b"\x1b[Z", cx);
    }

    /// A key that isn't typing, already encoded.
    fn send_key(&mut self, bytes: &'static [u8], cx: &mut Context<Self>) {
        self.clear_selection();
        self.recent.clear();
        self.typed();
        self.scroll_to_bottom();
        self.write(bytes);
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.pty.as_ref().and_then(|pty| pty.term.lock().selection_to_string()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn report(&mut self, action: MouseAction, position: Point<Pixels>, modifiers: &gpui::Modifiers) -> bool {
        let (col, row) = self.cell_at(position);
        match keys::mouse_bytes(action, col, row, modifiers, self.mode()) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let button = match event.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        // Cmd-click opens a link, and the program never sees the click.
        if event.modifiers.platform
            && button == 0
            && let Some(link) = self.link_at(event.position)
        {
            cx.open_url(&link.uri);
            cx.stop_propagation();
            return;
        }
        // Shift selects even while the program takes the mouse, as in Ghostty; with no program
        // asking for the mouse, any drag selects.
        if button == 0 && (event.modifiers.shift || !self.mode().intersects(TermMode::MOUSE_MODE)) {
            self.start_selection(event.position, event.click_count);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.clear_selection() {
            cx.notify();
        }
        self.pressed = Some(button);
        self.last_cell = Some(self.cell_at(event.position));
        if self.report(MouseAction::Press(button), event.position, &event.modifiers) {
            cx.stop_propagation();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            return;
        }
        if let Some(button) = self.pressed.take() {
            self.report(MouseAction::Release(button), event.position, &event.modifiers);
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some(event.position);
        if self.selecting {
            let (point, side) = self.grid_point(event.position);
            if let Some(pty) = &self.pty
                && let Some(selection) = pty.term.lock().selection.as_mut()
            {
                selection.update(point, side);
            }
            cx.notify();
            return;
        }
        let cell = self.cell_at(event.position);
        if self.last_cell == Some(cell) {
            return;
        }
        self.last_cell = Some(cell);
        self.update_link(event.modifiers.platform, cx);
        let action = match self.pressed {
            Some(button) => MouseAction::Drag(button),
            None => MouseAction::Move,
        };
        self.report(action, event.position, &event.modifiers);
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / f32::from(self.layout.cell.height.max(px(1.))),
        };
        self.wheel += lines;
        let steps = self.wheel.trunc() as i32;
        if steps == 0 {
            return;
        }
        self.wheel -= steps as f32;
        let mode = self.mode();
        for _ in 0..steps.unsigned_abs().min(8) {
            let action = if steps > 0 {
                MouseAction::WheelUp
            } else {
                MouseAction::WheelDown
            };
            if self.report(action, event.position, &event.modifiers) {
                continue;
            }
            if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
                let arrow: &'static [u8] = match (steps > 0, mode.contains(TermMode::APP_CURSOR)) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                self.write(arrow);
            } else if let Some(pty) = &self.pty {
                pty.term.lock().scroll_display(Scroll::Delta(steps.signum()));
            }
        }
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::back_tab))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_drop(cx.listener(Self::drop_paths))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                if !*hovered {
                    view.pointer = None;
                    view.update_link(false, cx);
                }
            }))
            .when(self.hovered_link.is_some(), |terminal| terminal.cursor(PointerStyle::PointingHand))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.border_color(rgb(theme::ACCENT)))
            .relative()
            .size_full()
            .border_1()
            .border_color(rgb(colors::BACKGROUND))
            .bg(rgb(colors::BACKGROUND))
            .px(px(12.))
            .py(px(10.))
            .child(TerminalElement::new(cx.entity()))
    }
}

/// macOS's text input: what was typed, an input method's composition, and the press-and-hold
/// accent menu. The terminal has no editable text of its own, so it answers from the
/// composition in progress, or else from what was just typed.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.marked.as_deref().unwrap_or(&self.recent);
        let units: Vec<u16> = text.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.marked.as_deref().unwrap_or(&self.recent).encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.marked.as_ref().map(|text| 0..text.encode_utf16().count())
    }

    /// The composition is accepted as it stands.
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.marked.take() {
            self.send_text(&text);
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A composition is replaced by its result; typed text by its replacement (an accent),
        // which means erasing it first, as Backspace would.
        if self.marked.take().is_none()
            && let Some(range) = range
        {
            let mut offset = 0;
            let erase = self
                .recent
                .chars()
                .filter(|c| {
                    let at = offset;
                    offset += c.len_utf16();
                    at >= range.start
                })
                .count();
            if erase > 0 {
                let keep = self.recent.chars().count() - erase;
                self.recent = self.recent.chars().take(keep).collect();
                self.write(vec![0x7f; erase]);
            }
        }
        self.send_text(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    fn screen(bytes: &[u8]) -> Term<VoidListener> {
        let size = GridSize { columns: 60, lines: 4 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut term, bytes);
        term
    }

    fn link(term: &Term<VoidListener>, col: usize, row: usize) -> Option<(usize, usize, String)> {
        let mut regex = RegexSearch::new(LINK_PATTERN).unwrap();
        link_in(term, col, row, &mut regex).map(|link| (link.start.column.0, link.end.column.0, link.uri))
    }

    #[test]
    fn finds_the_url_under_the_pointer_without_trailing_prose() {
        let term = screen(b"see (https://example.com/a?b=1), then go");
        let expected = Some((5, 29, "https://example.com/a?b=1".to_string()));
        assert_eq!(link(&term, 5, 0), expected);
        assert_eq!(link(&term, 20, 0), expected);
        // The closing bracket and comma aren't part of it, nor is the plain text.
        assert_eq!(link(&term, 30, 0), None);
        assert_eq!(link(&term, 1, 0), None);
        assert_eq!(link(&term, 5, 1), None);
    }

    /// Waits for `want` to appear on the terminal's screen, as a program's output arrives.
    fn wait_for(view: &gpui::Entity<TerminalView>, want: &str, cx: &mut gpui::VisualTestContext) -> String {
        let mut screen = String::new();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(20));
            cx.run_until_parked();
            screen = view.read_with(cx, |view, _| view.screen_text());
            if screen.contains(want) {
                break;
            }
        }
        screen
    }

    #[gpui::test]
    fn typing_accents_and_input_methods_reach_the_program(cx: &mut gpui::TestAppContext) {
        // A real program on a real pty: its output arrives from the pty's own thread.
        cx.executor().allow_parking();
        cx.update(|cx| cx.set_global(TerminalFont(DEFAULT_FONT_SIZE)));
        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::new(cx));
        view.update_in(cx, |view, window, cx| {
            view.start("/bin/cat", &[], cx).unwrap();
            window.focus(&view.focus_handle, cx);
        });
        cx.run_until_parked();

        // Plain typing arrives through text input; the tty echoes it.
        cx.simulate_input("hi e");
        assert!(wait_for(&view, "hi e", cx).contains("hi e"));

        // An accent from the press-and-hold menu replaces the letter just typed.
        view.update_in(cx, |view, window, cx| view.replace_text_in_range(Some(3..4), "é", window, cx));
        let screen = wait_for(&view, "hi é", cx);
        assert!(screen.contains("hi é") && !screen.contains("hi e"), "{screen:?}");

        // A composition shows at the cursor and reaches the program only when committed.
        view.update_in(cx, |view, window, cx| {
            view.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
        });
        assert_eq!(view.read_with(cx, |view, _| view.marked_text().map(str::to_string)), Some("にほ".into()));
        view.update_in(cx, |view, window, cx| view.replace_text_in_range(None, "日本", window, cx));
        assert_eq!(view.read_with(cx, |view, _| view.marked_text().map(str::to_string)), None);
        assert!(wait_for(&view, "hi é日本", cx).contains("hi é日本"));
    }

    #[gpui::test]
    fn a_program_can_put_text_on_the_clipboard(cx: &mut gpui::TestAppContext) {
        // How tmux hands over what was copied in it (OSC 52, exactly as tmux 3.5 sends it).
        cx.executor().allow_parking();
        cx.update(|cx| cx.set_global(TerminalFont(DEFAULT_FONT_SIZE)));
        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::new(cx));
        // The text, base64: "copied from tmux".
        let script = "printf '\\033]52;;Y29waWVkIGZyb20gdG11eA==\\007done'; exec cat";
        view.update_in(cx, |view, _, cx| {
            view.start("/bin/sh", &["-c".to_string(), script.to_string()], cx).unwrap();
        });
        assert!(wait_for(&view, "done", cx).contains("done"));
        assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("copied from tmux".to_string()));
    }

    #[gpui::test]
    fn shift_drag_selects_and_copy_takes_it(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            cx.set_global(TerminalFont(DEFAULT_FONT_SIZE));
            init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::new(cx));
        view.update_in(cx, |view, window, cx| {
            view.start("/bin/cat", &[], cx).unwrap();
            window.focus(&view.focus_handle, cx);
        });
        cx.run_until_parked();
        cx.simulate_input("alpha beta");
        assert!(wait_for(&view, "alpha beta", cx).contains("alpha beta"));

        // From the middle of "b" to the middle of "a" at the end of "beta": the cells between.
        let Layout { origin, cell } = view.read_with(cx, |view, _| view.layout);
        let at = |col: f32| gpui::point(origin.x + cell.width * col, origin.y + cell.height * 0.5);
        let shift = gpui::Modifiers::shift();
        cx.simulate_mouse_down(at(6.2), gpui::MouseButton::Left, shift);
        cx.simulate_mouse_move(at(9.8), gpui::MouseButton::Left, shift);
        cx.simulate_mouse_up(at(9.8), gpui::MouseButton::Left, shift);
        cx.dispatch_action(Copy);
        assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("beta".to_string()));

        // Typing clears it.
        cx.simulate_input("!");
        assert!(view.read_with(cx, |view, _| view.pty.as_ref().unwrap().term.lock().selection.is_none()));

        // Tab reaches the program, though the kit binds it to moving focus. On a fresh line
        // (after cat repeats the first), the tab stop puts "y" at column 8; the terminal keeps
        // the tab itself in the cell after "x".
        cx.simulate_keystrokes("enter");
        wait_for(&view, "alpha beta!\nalpha beta!", cx);
        cx.simulate_keystrokes("x tab y");
        let screen = wait_for(&view, "\nx\t      y", cx);
        assert!(screen.contains("\nx\t      y"), "{screen:?}");
    }

    #[test]
    fn a_programs_own_hyperlink_wins() {
        let term = screen(b"open \x1b]8;;https://docs.rs/gpui\x1b\\the docs\x1b]8;;\x1b\\ now");
        assert_eq!(link(&term, 7, 0), Some((5, 12, "https://docs.rs/gpui".to_string())));
        assert_eq!(link(&term, 14, 0), None);
    }
}
