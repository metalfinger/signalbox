//! The column on the right: the images and videos the Claude session in the active tab made or
//! looked at. Clicking one opens it over that tab's terminal; a video opens in QuickTime.

use gpui::{AnyElement, Entity, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px, rgb, rgba};
use gpui_kit::component::TitleBar;

use crate::media::{Kind, MediaItem};
use crate::theme;
use crate::tmux_tab::TmuxTab;

pub const WIDTH: f32 = 300.;

pub fn panel(tab: &Entity<TmuxTab>, media: &[MediaItem]) -> AnyElement {
    let mut grid = div().flex().flex_wrap().gap_2();
    for item in media {
        let name = item
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let picture = match &item.thumb {
            Some(thumb) => img(thumb.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(rgb(theme::TEXT_FAINT))
                .child("…")
                .into_any_element(),
        };
        let (tab, clicked) = (tab.clone(), item.clone());
        grid = grid.child(
            div()
                .id(SharedString::from(format!("media-{}", item.path.display())))
                .w(px(128.))
                .flex()
                .flex_col()
                .gap_1()
                .cursor_pointer()
                .child(
                    div()
                        .relative()
                        .w(px(128.))
                        .h(px(96.))
                        .rounded_md()
                        .overflow_hidden()
                        .bg(rgb(theme::SURFACE))
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .hover(|style| style.border_color(rgb(theme::ACCENT)))
                        .child(picture)
                        .children((item.kind == Kind::Video).then(|| {
                            div()
                                .absolute()
                                .bottom_1()
                                .left_1()
                                .px_1()
                                .rounded_sm()
                                .bg(rgba(0x0000_00b0))
                                .text_xs()
                                .text_color(rgb(theme::TEXT))
                                .child("▶ video")
                        })),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(name),
                )
                .on_click(move |_, window, cx| tab.update(cx, |tab, cx| tab.open(&clicked, window, cx))),
        );
    }
    let body = if media.is_empty() {
        div()
            .text_xs()
            .text_color(rgb(theme::TEXT_FAINT))
            .child("No images or videos from this session yet.")
            .into_any_element()
    } else {
        grid.into_any_element()
    };
    let label = if media.is_empty() {
        "Media".to_string()
    } else {
        format!("Media · {}", media.len())
    };
    div()
        .w(px(WIDTH))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .border_l_1()
        .border_color(rgb(theme::BORDER))
        .child(
            TitleBar::new().h(px(40.)).pl(px(16.)).border_b_0().child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(label),
            ),
        )
        .child(
            div()
                .id("media-panel")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_4()
                .pb_4()
                .child(body),
        )
        .into_any_element()
}
