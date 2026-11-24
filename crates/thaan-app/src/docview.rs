//! MongoDB result views: Table · Tree · JSON (docs/spec/mongodb-ui.md M1–M5).
//!
//! The table is the regular result grid; this module draws the tree and JSON views from
//! `ResultSet::documents` and owns the switch between the three.

use gpui_kit::base::{Selectable as _, StyledExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use thaan_core::doc::{Doc, DocKind};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum DocView {
    #[default]
    Table,
    Tree,
    Json,
}

/// Value colour per BSON family. ThaanDB's own mapping onto theme tokens.
pub fn kind_color(kind: DocKind, theme: &Theme) -> Hsla {
    match kind {
        DocKind::String => theme.foreground,
        DocKind::Number => theme.cyan,
        DocKind::Bool => theme.yellow,
        DocKind::Date => theme.blue,
        DocKind::Id => theme.muted_foreground,
        DocKind::Object | DocKind::Array => theme.primary,
        DocKind::Null => theme.muted_foreground.opacity(0.7),
        DocKind::Other => theme.magenta,
    }
}

pub struct DocPanel {
    view: DocView,
    docs: Vec<Doc>,
    /// Paths of expanded tree rows, e.g. `"0"`, `"0/address"`, `"0/orders/1"`.
    expanded: HashSet<String>,
    json: Entity<EditorState>,
    json_stale: bool,
}

struct TreeRow {
    path: String,
    depth: usize,
    key: String,
    value: String,
    type_name: String,
    kind: DocKind,
    container: bool,
    open: bool,
}

impl DocPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let json = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("javascript")
                .line_number(true)
        });
        Self {
            view: DocView::Table,
            docs: Vec::new(),
            expanded: HashSet::new(),
            json,
            json_stale: false,
        }
    }

    pub fn view(&self) -> DocView {
        self.view
    }

    pub fn set_view(&mut self, view: DocView, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    pub fn set_docs(&mut self, docs: Vec<Doc>, cx: &mut Context<Self>) {
        self.docs = docs;
        self.expanded.clear();
        self.json_stale = true;
        cx.notify();
    }

    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
        cx.notify();
    }

    fn json_text(&self) -> String {
        self.docs
            .iter()
            .enumerate()
            .map(|(i, d)| format!("// {}\n{}", i + 1, d.to_pretty()))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn rows(&self) -> Vec<TreeRow> {
        fn push(
            out: &mut Vec<TreeRow>,
            expanded: &HashSet<String>,
            path: String,
            depth: usize,
            key: String,
            d: &Doc,
            top: bool,
        ) {
            let open = expanded.contains(&path);
            out.push(TreeRow {
                value: d.summary(),
                type_name: if top {
                    "Document".into()
                } else {
                    d.type_name().to_string()
                },
                kind: d.kind(),
                container: d.is_container(),
                open,
                key,
                depth,
                path: path.clone(),
            });
            if open {
                for (k, child) in d.children() {
                    push(
                        out,
                        expanded,
                        format!("{path}/{k}"),
                        depth + 1,
                        k,
                        child,
                        false,
                    );
                }
            }
        }
        let mut out = Vec::new();
        for (i, d) in self.docs.iter().enumerate() {
            let label = match d.get("_id") {
                Some(id) => format!("{}   _id: {}", i + 1, id.summary()),
                None => format!("{}", i + 1),
            };
            push(&mut out, &self.expanded, i.to_string(), 0, label, d, true);
        }
        out
    }

    fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        const TYPE_W: f32 = 110.;
        let header = h_flex()
            .h(px(28.))
            .px_2()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(relative(0.4)).child("Field"))
            .child(div().flex_1().child("Value"))
            .child(div().w(px(TYPE_W)).child("Type"));
        let rows = self.rows();
        let list = v_flex()
            .id("doc-tree")
            .flex_1()
            .overflow_y_scroll()
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                let path = r.path.clone();
                let chevron = if r.open {
                    "chevron-down"
                } else {
                    "chevron-right"
                };
                h_flex()
                    .id(SharedString::from(format!("doc-row-{}", r.path)))
                    .h(px(26.))
                    .px_2()
                    .gap_2()
                    .text_sm()
                    .when(i % 2 == 1, |d| d.bg(theme.table_even))
                    .hover(|s| s.bg(theme.table_hover))
                    .when(r.container, |d| d.cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&path, cx)))
                    .child(
                        h_flex()
                            .w(relative(0.4))
                            .min_w_0()
                            .gap_1()
                            .pl(px(r.depth as f32 * 16.))
                            .child(div().w(px(14.)).flex_none().when(r.container, |d| {
                                d.child(
                                    Icon::empty()
                                        .path(SharedString::from(format!("icons/{chevron}.svg")))
                                        .size_3p5()
                                        .text_color(theme.muted_foreground),
                                )
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .when(r.depth == 0, |d| d.font_medium())
                                    .child(r.key),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_color(kind_color(r.kind, &theme))
                            .child(r.value),
                    )
                    .child(
                        div()
                            .w(px(TYPE_W))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(r.type_name),
                    )
            }));
        v_flex()
            .size_full()
            .child(header)
            .child(list)
            .into_any_element()
    }
}

impl Render for DocPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.view {
            DocView::Json => {
                if self.json_stale {
                    let text = self.json_text();
                    self.json.update(cx, |e, cx| e.set_value(text, window, cx));
                    self.json_stale = false;
                }
                div()
                    .size_full()
                    .child(
                        Editor::new(&self.json)
                            .readonly(true)
                            .bordered(false)
                            .h_full(),
                    )
                    .into_any_element()
            }
            DocView::Tree => self.render_tree(cx),
            DocView::Table => div().into_any_element(),
        }
    }
}

/// Segmented Table · Tree · JSON switch for a panel.
pub fn view_switch(panel: &Entity<DocPanel>, cx: &App) -> impl IntoElement {
    let current = panel.read(cx).view();
    h_flex().gap_0p5().children(
        [
            (DocView::Table, "Table"),
            (DocView::Tree, "Tree"),
            (DocView::Json, "JSON"),
        ]
        .map(|(v, label)| {
            let p = panel.clone();
            Button::new(SharedString::from(format!("docview-{label}")))
                .xsmall()
                .ghost()
                .label(label)
                .selected(current == v)
                .on_click(move |_, _, cx| p.update(cx, |p, cx| p.set_view(v, cx)))
        }),
    )
}
