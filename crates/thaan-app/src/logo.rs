//! The ThaanDB mark ("Table T", see docs/brand/README.md), drawn from cells so it stays crisp
//! and takes its colours from the theme: header row in the accent, column in the text colour.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

/// 14 px mark on the small-size grid: 4 px cells, 1 px gaps.
pub fn mark(header: Hsla, column: Hsla) -> impl IntoElement {
    let cell = |c: Hsla| div().flex_none().size(px(4.)).rounded(px(1.)).bg(c);
    let stem = |c: Hsla| h_flex().pl(px(5.)).child(cell(c));
    v_flex()
        .flex_none()
        .gap(px(1.))
        .child(
            h_flex()
                .gap(px(1.))
                .child(cell(header))
                .child(cell(header))
                .child(cell(header)),
        )
        .child(stem(column))
        .child(stem(column))
}
