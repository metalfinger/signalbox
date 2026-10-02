//! A plain button, for the few places the kit's would be too much.

use gpui::{Div, ElementId, FontWeight, InteractiveElement, ParentElement, SharedString, Stateful, Styled, div, rgb};

use crate::theme;

#[derive(Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Quiet,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Stateful<Div> {
    let (bg, fg, border, hover) = match kind {
        ButtonKind::Primary => (theme::ACCENT, theme::ON_ACCENT, theme::ACCENT, theme::ACCENT_HOVER),
        ButtonKind::Quiet => (theme::SURFACE, theme::TEXT, theme::BORDER, theme::SURFACE_HOVER),
    };
    div()
        .id(id.into())
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(hover)))
        .child(label.into())
}
