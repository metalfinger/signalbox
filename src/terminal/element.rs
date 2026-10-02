//! Draws the terminal grid: one shaped line per row with glyphs held to the cell width,
//! background runs, and the cursor.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, BorderStyle, Bounds, Element, ElementId, ElementInputHandler, Entity, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine, StrikethroughStyle, Style, TextAlign, TextRun,
    UnderlineStyle, Window, fill, outline, point, px, relative, rgb, size,
};

use super::{GridSize, Layout, TerminalFont, TerminalView, colors};

const FONT_FAMILY: &str = "Hack Nerd Font Mono";
/// Ghostty's `adjust-cell-height = 10%`.
const CELL_HEIGHT_SCALE: f32 = 1.1;

fn terminal_font(weight: FontWeight, style: FontStyle) -> Font {
    Font {
        family: FONT_FAMILY.into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(vec!["Menlo".to_string()])),
        weight,
        style,
    }
}

pub struct TerminalElement {
    view: Entity<TerminalView>,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>) -> Self {
        Self { view }
    }
}

#[derive(Default)]
pub struct Frame {
    line_height: Pixels,
    rows: Vec<(Point<Pixels>, ShapedLine)>,
    backgrounds: Vec<PaintQuad>,
    /// Under the link the pointer is on while Cmd is held.
    link_underlines: Vec<PaintQuad>,
    /// An input method's composition, over the cursor: where, the text, what's behind it.
    marked: Option<(Point<Pixels>, ShapedLine, PaintQuad)>,
    /// A block cursor goes under the text; the other shapes go over it.
    cursor_under: Option<PaintQuad>,
    cursor_over: Option<PaintQuad>,
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: u32,
    weight: FontWeight,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

impl CellStyle {
    fn run(&self, len: usize) -> TextRun {
        let color = rgb(self.fg).into();
        TextRun {
            len,
            font: terminal_font(
                self.weight,
                if self.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
            ),
            color,
            background_color: None,
            underline: (self.underline || self.undercurl).then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: self.undercurl,
            }),
            strikethrough: self.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = px(cx.global::<TerminalFont>().0);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&terminal_font(FontWeight::NORMAL, FontStyle::Normal));
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(px(8.4));
        let line_height = ((text_system.ascent(font_id, font_size) + text_system.descent(font_id, font_size).abs())
            * CELL_HEIGHT_SCALE)
            .round();
        let grid = GridSize {
            columns: (bounds.size.width / cell_width).floor().max(1.) as usize,
            lines: (bounds.size.height / line_height).floor().max(1.) as usize,
        };
        let layout = Layout {
            origin: bounds.origin,
            cell: size(cell_width, line_height),
        };
        let focused = self.view.read(cx).focus_handle.is_focused(window) && window.is_window_active();
        self.view.update(cx, |view, _| {
            view.fit(grid, layout);
            let blinks = view
                .pty
                .as_ref()
                .is_some_and(|pty| pty.term.lock().cursor_style().blinking);
            view.set_cursor_state(focused, blinks);
        });

        let view = self.view.read(cx);
        let Some(pty) = &view.pty else {
            return Frame {
                line_height,
                ..Frame::default()
            };
        };
        let term = pty.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let cell_origin = |row: usize, col: usize| {
            point(
                bounds.left() + cell_width * col as f32,
                bounds.top() + line_height * row as f32,
            )
        };

        let cursor_row = content.cursor.point.line.0 + offset;
        let cursor_col = content.cursor.point.column.0;
        let shape = match content.cursor.shape {
            CursorShape::Hidden => CursorShape::Hidden,
            _ if !focused => CursorShape::HollowBlock,
            _ if term.cursor_style().blinking && !view.cursor_on() => CursorShape::Hidden,
            shape => shape,
        };
        let cursor_cell = (cursor_row >= 0 && (cursor_row as usize) < grid.lines && shape != CursorShape::Hidden)
            .then_some((cursor_row as usize, cursor_col));
        let cursor_color = rgb(colors::CURSOR);
        let mut frame = Frame {
            line_height,
            ..Frame::default()
        };
        if let Some((row, col)) = cursor_cell {
            let origin = cell_origin(row, col);
            let cell = Bounds::new(origin, size(cell_width, line_height));
            match shape {
                CursorShape::Block => frame.cursor_under = Some(fill(cell, cursor_color)),
                CursorShape::Beam => {
                    frame.cursor_over = Some(fill(Bounds::new(origin, size(px(2.), line_height)), cursor_color))
                }
                CursorShape::Underline => {
                    frame.cursor_over = Some(fill(
                        Bounds::new(
                            point(origin.x, origin.y + line_height - px(2.)),
                            size(cell_width, px(2.)),
                        ),
                        cursor_color,
                    ))
                }
                CursorShape::HollowBlock => frame.cursor_over = Some(outline(cell, cursor_color, BorderStyle::Solid)),
                CursorShape::Hidden => {}
            }
        }

        if let Some(link) = view.hovered_link() {
            for line in link.start.line.0..=link.end.line.0 {
                let row = line + offset;
                if row < 0 || row as usize >= grid.lines {
                    continue;
                }
                let first = if line == link.start.line.0 { link.start.column.0 } else { 0 };
                let last = if line == link.end.line.0 {
                    link.end.column.0
                } else {
                    grid.columns - 1
                };
                let origin = cell_origin(row as usize, first);
                frame.link_underlines.push(fill(
                    Bounds::new(
                        point(origin.x, origin.y + line_height - px(2.)),
                        size(cell_width * (last + 1).saturating_sub(first) as f32, px(1.)),
                    ),
                    rgb(colors::FOREGROUND),
                ));
            }
        }

        if let (Some(text), Some((row, col))) = (view.marked_text(), cursor_cell) {
            let style = CellStyle {
                fg: colors::FOREGROUND,
                weight: FontWeight::NORMAL,
                italic: false,
                underline: true,
                undercurl: false,
                strike: false,
            };
            let line = text_system.shape_line(text.to_string().into(), font_size, &[style.run(text.len())], None);
            let origin = cell_origin(row, col);
            let behind = fill(Bounds::new(origin, size(line.width, line_height)), rgb(colors::BACKGROUND));
            frame.marked = Some((origin, line, behind));
        }

        let mut row_text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut run_style: Option<CellStyle> = None;
        let mut run_len = 0;
        let mut background: Option<(usize, usize, u32)> = None;
        let mut current_row: Option<usize> = None;

        let finish_row = |row: usize,
                          text: &mut String,
                          runs: &mut Vec<TextRun>,
                          run_style: &mut Option<CellStyle>,
                          run_len: &mut usize,
                          background: &mut Option<(usize, usize, u32)>,
                          frame: &mut Frame| {
            if let Some(style) = run_style.take() {
                runs.push(style.run(*run_len));
            }
            *run_len = 0;
            if let Some((start, end, color)) = background.take() {
                frame.backgrounds.push(fill(
                    Bounds::new(
                        cell_origin(row, start),
                        size(cell_width * (end - start) as f32, line_height),
                    ),
                    rgb(color),
                ));
            }
            if !text.is_empty() {
                let line = text_system.shape_line(
                    std::mem::take(text).into(),
                    font_size,
                    &std::mem::take(runs),
                    Some(cell_width),
                );
                frame.rows.push((cell_origin(row, 0), line));
            }
        };

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= grid.lines {
                continue;
            }
            let row = row as usize;
            let col = indexed.point.column.0;
            if current_row != Some(row) {
                if let Some(previous) = current_row {
                    finish_row(
                        previous,
                        &mut row_text,
                        &mut runs,
                        &mut run_style,
                        &mut run_len,
                        &mut background,
                        &mut frame,
                    );
                }
                current_row = Some(row);
            }
            let cell = indexed.cell;
            let flags = cell.flags;
            let mut fg = colors::resolve(cell.fg, content.colors);
            let mut bg = colors::resolve(cell.bg, content.colors);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            // Claude Code's suggestions, among others.
            if flags.contains(Flags::DIM) {
                fg = colors::faint(fg, bg);
            }
            if content.selection.is_some_and(|range| range.contains(indexed.point)) {
                bg = colors::SELECTION;
            }
            if cursor_cell == Some((row, col)) && shape == CursorShape::Block {
                fg = colors::BACKGROUND;
            }

            if bg != colors::BACKGROUND {
                match &mut background {
                    Some((_, end, color)) if *end == col && *color == bg => *end = col + 1,
                    _ => {
                        if let Some((start, end, color)) = background.take() {
                            frame.backgrounds.push(fill(
                                Bounds::new(
                                    cell_origin(row, start),
                                    size(cell_width * (end - start) as f32, line_height),
                                ),
                                rgb(color),
                            ));
                        }
                        background = Some((col, col + 1, bg));
                    }
                }
            }

            let ch = if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };
            let style = CellStyle {
                fg,
                weight: if flags.contains(Flags::BOLD) {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES) && !flags.contains(Flags::UNDERCURL),
                undercurl: flags.contains(Flags::UNDERCURL),
                strike: flags.contains(Flags::STRIKEOUT),
            };
            if run_style != Some(style) {
                if let Some(previous) = run_style.replace(style) {
                    runs.push(previous.run(run_len));
                }
                run_len = 0;
            }
            row_text.push(ch);
            run_len += ch.len_utf8();
        }
        if let Some(row) = current_row {
            finish_row(
                row,
                &mut row_text,
                &mut runs,
                &mut run_style,
                &mut run_len,
                &mut background,
                &mut frame,
            );
        }
        frame
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        frame: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        for quad in frame.backgrounds.drain(..) {
            window.paint_quad(quad);
        }
        if let Some(cursor) = frame.cursor_under.take() {
            window.paint_quad(cursor);
        }
        for (origin, line) in &frame.rows {
            let _ = line.paint(*origin, frame.line_height, TextAlign::Left, None, window, cx);
        }
        for quad in frame.link_underlines.drain(..) {
            window.paint_quad(quad);
        }
        if let Some((origin, line, behind)) = frame.marked.take() {
            window.paint_quad(behind);
            let _ = line.paint(origin, frame.line_height, TextAlign::Left, None, window, cx);
        }
        if let Some(cursor) = frame.cursor_over.take() {
            window.paint_quad(cursor);
        }
    }
}
