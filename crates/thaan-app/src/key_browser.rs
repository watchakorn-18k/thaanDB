//! Redis key browser (docs/spec/table-view.md T6): one tab per database. On the left, a
//! pattern box, a type picker and the keys, loaded a batch at a time with `keys.scan`
//! (a cursor, never the whole keyspace). On the right, the picked key: its type, TTL and
//! size, and its contents in the result grid, a page at a time (`table.fetchPage`).
//! Every call runs off the UI thread; a late answer for an older scan or key is dropped.

use crate::grid::{track_focus, ResultGrid, GRID_HEADER_H};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;
use thaan_core::browse::{KeyEntry, KeyScan};
use thaan_core::engine::{Engine, SessionId};
use thaan_core::schema::ObjectRef;
use thaan_core::table::PageRequest;
use thaan_core::ResultSet;

/// Keys kept in the list; past this the list asks for a narrower pattern.
const KEY_CAP: usize = 10_000;
/// Keys a "load" aims for before it stops scanning.
const BATCH: usize = 200;
/// Rows per page in the key's grid.
const PAGE: u64 = 100;
const ROW_H: f32 = 28.;

const TYPES: [&str; 7] = [
    "All types",
    "string",
    "hash",
    "list",
    "set",
    "zset",
    "stream",
];

fn icon(name: &str) -> Icon {
    Icon::empty().path(SharedString::from(format!("icons/{name}.svg")))
}

/// `3 d`, `5 h`, `12 min`, `40 s` for a TTL in seconds.
pub(crate) fn ttl_text(ttl: Option<i64>) -> String {
    match ttl {
        None => "no expiry".into(),
        Some(s) if s >= 86_400 => format!("{} d", s / 86_400),
        Some(s) if s >= 3_600 => format!("{} h", s / 3_600),
        Some(s) if s >= 60 => format!("{} min", s / 60),
        Some(s) => format!("{s} s"),
    }
}

struct Detail {
    key: String,
    key_type: String,
    ttl: Option<i64>,
    total: u64,
    page: u64,
    loading: bool,
    error: Option<String>,
}

pub struct KeyBrowser {
    engine: Arc<Engine>,
    sid: SessionId,
    pattern: Entity<InputState>,
    kind: Entity<SelectState<Vec<SharedString>>>,
    keys: Vec<KeyEntry>,
    cursor: String,
    done: bool,
    total: Option<u64>,
    loading: bool,
    error: Option<String>,
    /// Bumped by every new scan, so answers for an older one are dropped.
    epoch: u64,
    selected: Option<usize>,
    detail: Option<Detail>,
    grid: Entity<TableState<ResultGrid>>,
    list: UniformListScrollHandle,
    read_only: bool,
}

impl KeyBrowser {
    pub fn new(
        engine: Arc<Engine>,
        sid: SessionId,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pattern = cx.new(|cx| InputState::new(window, cx).placeholder("Pattern, e.g. user:*"));
        cx.subscribe_in(&pattern, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.rescan(cx);
            }
        })
        .detach();
        let items: Vec<SharedString> = TYPES.iter().map(|t| SharedString::from(*t)).collect();
        let kind = cx.new(|cx| SelectState::new(items, Some(IndexPath::new(0)), window, cx));
        cx.subscribe_in(
            &kind,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
                this.rescan(cx);
            },
        )
        .detach();
        let grid = cx.new(|cx| TableState::new(ResultGrid::new(ResultSet::default()), window, cx));
        track_focus(&grid, cx);
        let mut this = Self {
            engine,
            sid,
            pattern,
            kind,
            keys: Vec::new(),
            cursor: "0".into(),
            done: false,
            total: None,
            loading: false,
            error: None,
            epoch: 0,
            selected: None,
            detail: None,
            grid,
            list: UniformListScrollHandle::new(),
            read_only,
        };
        this.rescan(cx);
        this
    }

    fn scan_params(&self, cx: &App) -> (Option<String>, Option<String>) {
        let p = self.pattern.read(cx).value().trim().to_string();
        let t = self
            .kind
            .read(cx)
            .selected_value()
            .map(|v| v.to_string())
            .filter(|v| v != TYPES[0]);
        ((!p.is_empty()).then_some(p), t)
    }

    /// Starts over from the first key with the current pattern and type.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        self.keys.clear();
        self.cursor = "0".into();
        self.done = false;
        self.selected = None;
        self.error = None;
        self.load_more(cx);
    }

    /// Scans on from the cursor until a batch of keys has come in or the scan ends.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.done || self.keys.len() >= KEY_CAP {
            return;
        }
        let (pattern, key_type) = self.scan_params(cx);
        let (engine, sid, epoch) = (self.engine.clone(), self.sid, self.epoch);
        let mut cursor = self.cursor.clone();
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let mut got = Vec::new();
                    let mut total = None;
                    // A selective pattern can return few keys per step: keep stepping,
                    // but not forever on a huge keyspace.
                    for _ in 0..50 {
                        let page = engine.scan_keys(
                            sid,
                            &KeyScan {
                                pattern: pattern.clone(),
                                key_type: key_type.clone(),
                                cursor: cursor.clone(),
                                count: BATCH as u32,
                            },
                        )?;
                        total = page.total.or(total);
                        let done = page.done();
                        got.extend(page.keys);
                        cursor = page.cursor;
                        if done || got.len() >= BATCH {
                            return Ok::<_, thaan_core::Error>((got, cursor, done, total));
                        }
                    }
                    Ok((got, cursor, false, total))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.loading = false;
                match res {
                    Ok((keys, cursor, done, total)) => {
                        this.keys.extend(keys);
                        this.keys.truncate(KEY_CAP);
                        this.cursor = cursor;
                        this.done = done;
                        this.total = total.or(this.total);
                        if this.selected.is_none() && !this.keys.is_empty() && this.detail.is_none()
                        {
                            this.select(0, cx);
                        }
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(k) = self.keys.get(ix).cloned() else {
            return;
        };
        self.selected = Some(ix);
        self.detail = Some(Detail {
            key: k.name.clone(),
            key_type: k.key_type.clone(),
            ttl: k.ttl,
            total: 0,
            page: 0,
            loading: true,
            error: None,
        });
        self.load_page(0, cx);
    }

    fn load_page(&mut self, page: u64, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        d.page = page;
        d.loading = true;
        let key = d.key.clone();
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let k = key.clone();
            let res = cx
                .background_spawn(async move {
                    let ttl = engine
                        .driver_command(sid, &format!("TTL {}", thaan_core::cmdline::quote_arg(&k)))
                        .ok()
                        .and_then(|r| r.rows.first()?.first().cloned());
                    let page = engine.fetch_page(
                        sid,
                        &PageRequest {
                            table: ObjectRef::new(k),
                            filters: vec![],
                            raw_where: None,
                            projection: None,
                            raw_sort: None,
                            sort: vec![],
                            page,
                            page_size: PAGE,
                        },
                    );
                    (ttl, page)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(d) = this
                    .detail
                    .as_mut()
                    .filter(|d| d.key == key && d.page == page)
                else {
                    return;
                };
                d.loading = false;
                let (ttl, page) = res;
                if let Some(thaan_core::Value::Int(t)) = ttl {
                    d.ttl = (t >= 0).then_some(t);
                }
                match page {
                    Ok(p) => {
                        d.total = p.total_rows;
                        d.error = None;
                        let offset = page_offset(p.page, p.page_size);
                        this.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(p.result);
                            g.delegate_mut().set_row_offset(offset);
                            g.refresh(cx);
                        });
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks first, then deletes the key in the background and drops it from the list.
    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_ref() else {
            return;
        };
        let key = d.key.clone();
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let key = key.clone();
            alert
                .confirm()
                .title(SharedString::from(format!("Delete “{key}”?")))
                .description(format!(
                    "Runs DEL {}. The key and its value are removed and cannot be restored.",
                    thaan_core::cmdline::quote_arg(&key)
                ))
                .ok_text("Delete key")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, _, cx| {
                    this.update(cx, |b, cx| b.delete_key(key.clone(), cx)).ok();
                    true
                })
        });
    }

    fn delete_key(&mut self, key: String, cx: &mut Context<Self>) {
        let (engine, sid) = (self.engine.clone(), self.sid);
        let k = key.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine
                        .driver_command(sid, &format!("DEL {}", thaan_core::cmdline::quote_arg(&k)))
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        if let Some(ix) = this.keys.iter().position(|e| e.name == key) {
                            this.keys.remove(ix);
                        }
                        this.total = this.total.map(|t| t.saturating_sub(1));
                        this.detail = None;
                        this.selected = None;
                        this.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(ResultSet::default());
                            g.refresh(cx);
                        });
                    }
                    Err(e) => {
                        if let Some(d) = this.detail.as_mut() {
                            d.error = Some(e.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = match (self.total, self.done) {
            (_, true) => format!("{} keys", self.keys.len()),
            (Some(t), _) => format!("{} of {t} keys", self.keys.len()),
            (None, _) => format!("{} keys", self.keys.len()),
        };
        let header = v_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.pattern)
                                .small()
                                .prefix(icon("search").size_3p5()),
                        ),
                    )
                    .child(
                        Button::new("rescan")
                            .icon(icon("refresh-cw"))
                            .ghost()
                            .small()
                            .tooltip("Scan again")
                            .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(136.))
                            .flex_none()
                            .child(Select::new(&self.kind).small()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(count),
                    ),
            );
        let selected = self.selected;
        let rows = self.keys.len();
        let list = if rows == 0 {
            let note = if self.loading {
                "Scanning keys…".to_string()
            } else if let Some(e) = &self.error {
                e.clone()
            } else {
                "No keys match".to_string()
            };
            div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(if self.error.is_some() {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(note)
                .into_any_element()
        } else {
            let view = cx.entity().downgrade();
            uniform_list("keys", rows, move |range, _, cx| {
                let Some(view) = view.upgrade() else {
                    return Vec::new();
                };
                let b = view.read(cx);
                let theme = cx.theme();
                range
                    .filter_map(|ix| {
                        let k = b.keys.get(ix)?;
                        let on = selected == Some(ix);
                        let v = view.downgrade();
                        Some(
                            h_flex()
                                .id(("key", ix))
                                .w_full()
                                .h(px(ROW_H))
                                .px_3()
                                .gap_2()
                                .text_sm()
                                .cursor_pointer()
                                .when(on, |d| d.bg(theme.foreground.opacity(0.06)))
                                .hover(|d| d.bg(theme.foreground.opacity(0.04)))
                                .child(
                                    div()
                                        .w(px(44.))
                                        .flex_none()
                                        .text_xs()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(k.key_type.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme.foreground)
                                        .child(k.name.clone()),
                                )
                                .when_some(k.ttl, |d, t| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .font_family(theme.mono_font_family.clone())
                                            .text_color(theme.muted_foreground)
                                            .child(ttl_text(Some(t))),
                                    )
                                })
                                .on_click(move |_, _, cx| {
                                    v.update(cx, |b, cx| b.select(ix, cx)).ok();
                                }),
                        )
                    })
                    .collect()
            })
            .track_scroll(&self.list)
            .flex_1()
            .into_any_element()
        };
        let more = (!self.done && !self.keys.is_empty() && self.keys.len() < KEY_CAP).then(|| {
            div().p_2().border_t_1().border_color(theme.border).child(
                Button::new("more")
                    .label(if self.loading {
                        "Loading keys…"
                    } else {
                        "Load more"
                    })
                    .ghost()
                    .small()
                    .w_full()
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
            )
        });
        let capped = (self.keys.len() >= KEY_CAP).then(|| {
            div()
                .p_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "First {KEY_CAP} keys shown. Narrow the pattern to see others"
                ))
        });
        v_flex()
            .size_full()
            .child(header)
            .child(list)
            .children(more)
            .children(capped)
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(d) = self.detail.as_ref() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if self.loading {
                    "Scanning keys…"
                } else {
                    "Pick a key to see its value"
                })
                .into_any_element();
        };
        let pages = d.total.div_ceil(PAGE).max(1);
        let unit = match d.key_type.as_str() {
            "string" => "value",
            "hash" => "fields",
            "list" => "items",
            "set" | "zset" => "members",
            "stream" => "entries",
            _ => "rows",
        };
        let meta = format!("{} · {} {unit} · {}", d.key_type, d.total, ttl_text(d.ttl));
        let header = h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(d.key.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(meta),
                    ),
            )
            .child(
                Button::new("copy-key")
                    .icon(icon("clipboard"))
                    .ghost()
                    .small()
                    .tooltip("Copy key name")
                    .on_click({
                        let k = d.key.clone();
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(k.clone()))
                    }),
            )
            .child(
                Button::new("reload-key")
                    .icon(icon("refresh-cw"))
                    .ghost()
                    .small()
                    .tooltip("Reload")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let p = this.detail.as_ref().map_or(0, |d| d.page);
                        this.load_page(p, cx)
                    })),
            )
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("delete-key")
                        .icon(icon("trash"))
                        .ghost()
                        .small()
                        .tooltip("Delete key")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                        ),
                )
            });
        let status = h_flex()
            .h(px(32.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_1().child(match (&d.error, d.loading) {
                (Some(e), _) => div().text_color(theme.danger).child(e.clone()),
                (None, true) => div().child(format!("Loading {PAGE} rows…")),
                (None, false) => div().child(format!("Page {} of {pages}", d.page + 1)),
            }))
            .child(
                Button::new("prev")
                    .icon(icon("chevron-left"))
                    .ghost()
                    .xsmall()
                    .disabled(d.page == 0 || d.loading)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let p = this.detail.as_ref().map_or(0, |d| d.page.saturating_sub(1));
                        this.load_page(p, cx)
                    })),
            )
            .child(
                Button::new("next")
                    .icon(icon("chevron-right"))
                    .ghost()
                    .xsmall()
                    .disabled(d.page + 1 >= pages || d.loading)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let p = this.detail.as_ref().map_or(0, |d| d.page + 1);
                        this.load_page(p, cx)
                    })),
            );
        v_flex()
            .size_full()
            .child(header)
            .child(
                div().flex_1().overflow_hidden().child(
                    DataTable::new(&self.grid)
                        .stripe(true)
                        .bordered(false)
                        .header_row_height(px(GRID_HEADER_H)),
                ),
            )
            .child(status)
            .into_any_element()
    }
}

fn page_offset(page: u64, size: u64) -> u64 {
    page * size
}

impl Render for KeyBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        h_flex()
            .size_full()
            .child(
                div()
                    .w(px(320.))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(border)
                    .child(self.render_list(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_detail(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::ttl_text;

    #[test]
    fn ttls_read_short() {
        assert_eq!(ttl_text(None), "no expiry");
        assert_eq!(ttl_text(Some(45)), "45 s");
        assert_eq!(ttl_text(Some(600)), "10 min");
        assert_eq!(ttl_text(Some(7_300)), "2 h");
        assert_eq!(ttl_text(Some(200_000)), "2 d");
    }
}
