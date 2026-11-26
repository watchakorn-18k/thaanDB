//! Redis key browser (docs/spec/table-view.md T6): one tab per database.
//!
//! Left: the keys as a tree grouped by `:` (`user:1` sits in folder `user`), each with its
//! type and TTL; checkboxes for bulk delete; a pattern box with *Fuzzy* and *No expiry*
//! toggles and a type picker. Keys stream in with `keys.scan` until the scan ends or
//! [`KEY_CAP`] keys are kept, so a small database shows complete and a huge one stays light.
//!
//! Right: the picked key: type, size and TTL, its items a page at a time
//! (`table.fetchPage`), an item search for hashes, sets and sorted sets, and a row to add an
//! item (push, add field, add member). Every call runs off the UI thread; a late answer for
//! an older scan or key is dropped.

use crate::grid::{track_focus, ResultGrid, GRID_HEADER_H};
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Icon, IndexPath, Sizable as _, ThemeColor, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::sync::Arc;
use thaan_core::browse::{KeyEntry, KeyScan};
use thaan_core::cmdline::quote_arg;
use thaan_core::decode::{decode, Decoder};
use thaan_core::engine::{Engine, SessionId};
use thaan_core::schema::ObjectRef;
use thaan_core::table::PageRequest;
use thaan_core::ResultSet;

/// Keys kept in the list; past this the list asks for a narrower pattern.
const KEY_CAP: usize = 10_000;
/// `COUNT` hint per scan step.
const BATCH: u32 = 500;
/// Rows per page in the key's grid.
const PAGE: u64 = 100;
const ROW_H: f32 = 28.;
/// Indent per tree level.
const INDENT: f32 = 16.;
const SEP: char = ':';

const TYPES: [&str; 7] = [
    "All types",
    "string",
    "hash",
    "list",
    "set",
    "zset",
    "stream",
];
/// Types a new key can have (streams start from the query editor with XADD).
const NEW_TYPES: [&str; 5] = ["string", "hash", "list", "set", "zset"];

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

/// One of our theme's base hues per key type, so types tell apart at a glance.
fn type_color(theme: &ThemeColor, t: &str) -> Hsla {
    match t {
        "string" => theme.cyan,
        "hash" => theme.blue,
        "list" => theme.green,
        "set" => theme.yellow,
        "zset" => theme.magenta,
        "stream" => theme.red,
        _ => theme.muted_foreground,
    }
}

/// A small outlined label: the colour on the text and a faint border of the same.
fn badge(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex_none()
        .h(px(18.))
        .px_1p5()
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(color.opacity(0.4))
        .text_color(color)
        .text_xs()
        .child(text.into())
}

/// One row of the flattened key tree.
#[derive(Debug, Clone, PartialEq)]
enum TreeRow {
    Folder {
        /// `user:profile`, the prefix without the trailing separator.
        path: String,
        name: String,
        depth: usize,
        /// Indexes into `keys` of every key under this folder.
        keys: Vec<usize>,
        open: bool,
    },
    Key {
        ix: usize,
        label: String,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    folders: BTreeMap<String, Node>,
    leaves: Vec<(String, usize)>,
    all: Vec<usize>,
}

/// Groups key names by `:` into folders (sorted, folders before keys) and flattens the
/// open part of the tree into rows. `keep` says which keys show.
fn build_tree(
    names: &[&str],
    keep: impl Fn(usize) -> bool,
    open: &BTreeSet<String>,
) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (ix, name) in names.iter().enumerate() {
        if !keep(ix) {
            continue;
        }
        let parts: Vec<&str> = name.split(SEP).collect();
        let mut node = &mut root;
        node.all.push(ix);
        for p in &parts[..parts.len() - 1] {
            node = node.folders.entry((*p).to_string()).or_default();
            node.all.push(ix);
        }
        node.leaves.push((parts[parts.len() - 1].to_string(), ix));
    }
    fn walk(
        node: &Node,
        prefix: &str,
        depth: usize,
        open: &BTreeSet<String>,
        out: &mut Vec<TreeRow>,
    ) {
        for (name, child) in &node.folders {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}{SEP}{name}")
            };
            let is_open = open.contains(&path);
            out.push(TreeRow::Folder {
                path: path.clone(),
                name: name.clone(),
                depth,
                keys: child.all.clone(),
                open: is_open,
            });
            if is_open {
                walk(child, &path, depth + 1, open, out);
            }
        }
        let mut leaves = node.leaves.clone();
        leaves.sort();
        for (label, ix) in leaves {
            out.push(TreeRow::Key { ix, label, depth });
        }
    }
    let mut out = Vec::new();
    walk(&root, "", 0, open, &mut out);
    out
}

struct Detail {
    key: String,
    key_type: String,
    ttl: Option<i64>,
    total: u64,
    page: u64,
    loading: bool,
    error: Option<String>,
    /// The outcome of the last add, shown in the add row.
    note: Option<(String, bool)>,
}

pub struct KeyBrowser {
    engine: Arc<Engine>,
    sid: SessionId,
    /// The database's name (`db0`), for the command prompt.
    db: String,
    read_only: bool,
    pattern: Entity<InputState>,
    fuzzy: bool,
    no_expiry: bool,
    kind: Entity<SelectState<Vec<SharedString>>>,
    keys: Vec<KeyEntry>,
    cursor: String,
    done: bool,
    total: Option<u64>,
    loading: bool,
    error: Option<String>,
    /// Bumped by every new scan, so answers for an older one are dropped.
    epoch: u64,
    open: BTreeSet<String>,
    rows: Vec<TreeRow>,
    checked: HashSet<String>,
    detail: Option<Detail>,
    grid: Entity<TableState<ResultGrid>>,
    list: UniformListScrollHandle,
    // Item search and add row (right pane).
    item_search: Entity<InputState>,
    item_a: Entity<InputState>,
    item_b: Entity<InputState>,
    push_head: bool,
    /// Placeholders for the add row, set when a key of another type is picked.
    placeholders: Option<(&'static str, &'static str)>,
    // Add key dialog.
    new_name: Entity<InputState>,
    new_value: Entity<InputState>,
    new_field: Entity<InputState>,
    new_ttl: Entity<InputState>,
    new_type: usize,
    new_error: Option<String>,
    // Rename key dialog.
    rename_input: Entity<InputState>,
    rename_error: Option<String>,
    // Item dialog (double-click a row).
    item: Option<ItemView>,
    // Set expiry dialog.
    ttl_input: Entity<InputState>,
    ttl_error: Option<String>,
    // Command line tab.
    right_tab: RightTab,
    cmd_input: Entity<InputState>,
    cmd_log: VecDeque<CmdEntry>,
    cmd_busy: bool,
    cmd_scroll: ScrollHandle,
    /// The grid row last selected, for *Open item*.
    picked_row: Option<usize>,
    item_editor: Entity<EditorState>,
}

impl KeyBrowser {
    pub fn new(
        engine: Arc<Engine>,
        sid: SessionId,
        db: String,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let pattern = input("Pattern, e.g. user:*", window, cx);
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
        let item_search = input("Search items", window, cx);
        cx.subscribe_in(&item_search, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.load_page(0, cx);
            }
        })
        .detach();
        let item_a = input("Value", window, cx);
        cx.subscribe_in(&item_a, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.add_item(window, cx);
            }
        })
        .detach();
        let item_b = input("Value", window, cx);
        cx.subscribe_in(&item_b, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.add_item(window, cx);
            }
        })
        .detach();
        let grid = cx.new(|cx| TableState::new(ResultGrid::new(ResultSet::default()), window, cx));
        track_focus(&grid, cx);
        cx.subscribe_in(
            &grid,
            window,
            |this, _, ev: &TableEvent, window, cx| match ev {
                TableEvent::DoubleClickedCell(row, _) => this.open_item(*row, window, cx),
                TableEvent::SelectCell(row, _) | TableEvent::SelectRow(row) => {
                    this.picked_row = Some(*row);
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        let cmd_input = input("", window, cx);
        cx.subscribe_in(
            &cmd_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.run_command(window, cx);
                }
            },
        )
        .detach();
        let mut this = Self {
            engine,
            sid,
            db,
            read_only,
            pattern,
            fuzzy: false,
            no_expiry: false,
            kind,
            keys: Vec::new(),
            cursor: "0".into(),
            done: false,
            total: None,
            loading: false,
            error: None,
            epoch: 0,
            open: BTreeSet::new(),
            rows: Vec::new(),
            checked: HashSet::new(),
            detail: None,
            grid,
            list: UniformListScrollHandle::new(),
            item_search,
            item_a,
            item_b,
            push_head: false,
            placeholders: None,
            new_name: input("Key name, e.g. user:42", window, cx),
            new_value: input("Value", window, cx),
            new_field: input("Field", window, cx),
            new_ttl: input("Seconds (optional)", window, cx),
            new_type: 0,
            new_error: None,
            rename_input: input("New name", window, cx),
            rename_error: None,
            item: None,
            picked_row: None,
            ttl_input: input("Seconds", window, cx),
            ttl_error: None,
            right_tab: RightTab::Key,
            cmd_input: cmd_input.clone(),
            cmd_log: VecDeque::new(),
            cmd_busy: false,
            cmd_scroll: ScrollHandle::new(),
            item_editor: cx.new(|cx| {
                EditorState::new(window, cx)
                    .line_number(false)
                    .folding(false)
                    .soft_wrap(true)
            }),
        };
        this.rescan(cx);
        this
    }

    // ---- keys -----------------------------------------------------------------------

    fn scan_params(&self, cx: &App) -> (Option<String>, Option<String>) {
        let p = self.pattern.read(cx).value().trim().to_string();
        // Fuzzy: a plain word matches anywhere in the key.
        let p = if self.fuzzy && !p.is_empty() && !p.contains(['*', '?', '[']) {
            format!("*{p}*")
        } else {
            p
        };
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
        self.rows.clear();
        self.cursor = "0".into();
        self.done = false;
        self.error = None;
        self.load_more(cx);
    }

    fn rebuild(&mut self) {
        let names: Vec<&str> = self.keys.iter().map(|k| k.name.as_str()).collect();
        let keys = &self.keys;
        let no_expiry = self.no_expiry;
        self.rows = build_tree(
            &names,
            |ix| !no_expiry || keys[ix].ttl.is_none(),
            &self.open,
        );
    }

    /// One scan step in the background; it calls itself again until the scan ends or
    /// [`KEY_CAP`] keys are kept, so the list fills in while it loads.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.done || self.keys.len() >= KEY_CAP {
            self.loading = false;
            return;
        }
        let (pattern, key_type) = self.scan_params(cx);
        let (engine, sid, epoch) = (self.engine.clone(), self.sid, self.epoch);
        let cursor = self.cursor.clone();
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine.scan_keys(
                        sid,
                        &KeyScan {
                            pattern,
                            key_type,
                            cursor,
                            count: BATCH,
                        },
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                match res {
                    Ok(page) => {
                        this.done = page.done();
                        this.cursor = page.cursor;
                        this.total = page.total.or(this.total);
                        this.keys.extend(page.keys);
                        this.keys.truncate(KEY_CAP);
                        this.rebuild();
                        if this.detail.is_none() {
                            if let Some(TreeRow::Key { ix, .. }) =
                                this.rows.iter().find(|r| matches!(r, TreeRow::Key { .. }))
                            {
                                let ix = *ix;
                                this.select(ix, cx);
                            }
                        }
                        this.load_more(cx);
                    }
                    Err(e) => {
                        this.loading = false;
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.open.remove(&path) {
            self.open.insert(path);
        }
        self.rebuild();
        cx.notify();
    }

    /// Checks every key in `ixs`, or unchecks them all when they already are.
    fn toggle_checked(&mut self, ixs: &[usize], cx: &mut Context<Self>) {
        let names: Vec<String> = ixs
            .iter()
            .filter_map(|i| self.keys.get(*i))
            .map(|k| k.name.clone())
            .collect();
        if names.iter().all(|n| self.checked.contains(n)) {
            for n in names {
                self.checked.remove(&n);
            }
        } else {
            self.checked.extend(names);
        }
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.checked.is_empty() {
            let keys = &self.keys;
            let no_expiry = self.no_expiry;
            self.checked = keys
                .iter()
                .filter(|k| !no_expiry || k.ttl.is_none())
                .map(|k| k.name.clone())
                .collect();
        } else {
            self.checked.clear();
        }
        cx.notify();
    }

    // ---- the picked key -------------------------------------------------------------

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(k) = self.keys.get(ix).cloned() else {
            return;
        };
        self.detail = Some(Detail {
            key: k.name.clone(),
            key_type: k.key_type.clone(),
            ttl: k.ttl,
            total: 0,
            page: 0,
            loading: true,
            error: None,
            note: None,
        });
        let (a, b) = match k.key_type.as_str() {
            "hash" => ("Field", "Value"),
            "zset" => ("Score", "Member"),
            "set" => ("Member", ""),
            _ => ("Value", ""),
        };
        // Placeholders need the window; `render` applies them.
        self.placeholders = Some((a, b));
        self.load_page(0, cx);
    }

    fn load_page(&mut self, page: u64, cx: &mut Context<Self>) {
        let search = self.item_search.read(cx).value().trim().to_string();
        self.picked_row = None;
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        d.page = page;
        d.loading = true;
        let key = d.key.clone();
        // Hashes, sets and sorted sets filter with a MATCH glob; a plain word matches
        // anywhere.
        let raw_where = (!search.is_empty()
            && matches!(d.key_type.as_str(), "hash" | "set" | "zset"))
        .then(|| {
            if search.contains(['*', '?', '[']) {
                search
            } else {
                format!("*{search}*")
            }
        });
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let k = key.clone();
            let res = cx
                .background_spawn(async move {
                    let ttl = engine
                        .driver_command(sid, &format!("TTL {}", quote_arg(&k)))
                        .ok()
                        .and_then(|r| r.rows.first()?.first().cloned());
                    let page = engine.fetch_page(
                        sid,
                        &PageRequest {
                            table: ObjectRef::new(k),
                            filters: vec![],
                            raw_where,
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
                        let offset = p.page * p.page_size;
                        // One row (a string key): *Open item* needs no selection.
                        let single = p.result.rows.len() == 1;
                        this.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(p.result);
                            g.delegate_mut().set_row_offset(offset);
                            g.refresh(cx);
                        });
                        if single {
                            this.picked_row = Some(0);
                        }
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The add row: push to a list, add a hash field, a set member or a scored member.
    fn add_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_ref() else {
            return;
        };
        let a = self.item_a.read(cx).value().to_string();
        let b = self.item_b.read(cx).value().to_string();
        let k = quote_arg(&d.key);
        let cmd = match d.key_type.as_str() {
            "list" if !a.is_empty() => {
                format!(
                    "{} {k} {}",
                    if self.push_head { "LPUSH" } else { "RPUSH" },
                    quote_arg(&a)
                )
            }
            "hash" if !a.is_empty() => format!("HSET {k} {} {}", quote_arg(&a), quote_arg(&b)),
            "set" if !a.is_empty() => format!("SADD {k} {}", quote_arg(&a)),
            "zset" if !b.is_empty() => {
                if a.trim().parse::<f64>().is_err() {
                    if let Some(d) = self.detail.as_mut() {
                        d.note = Some(("Score must be a number".into(), true));
                    }
                    cx.notify();
                    return;
                }
                format!("ZADD {k} {} {}", a.trim(), quote_arg(&b))
            }
            _ => return,
        };
        self.item_a.update(cx, |s, cx| s.set_value("", window, cx));
        self.item_b.update(cx, |s, cx| s.set_value("", window, cx));
        let key = d.key.clone();
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.driver_command(sid, &cmd) })
                .await;
            this.update(cx, |this, cx| {
                let Some(d) = this.detail.as_mut().filter(|d| d.key == key) else {
                    return;
                };
                match res {
                    Ok(_) => {
                        d.note = Some(("Added".into(), false));
                        let p = d.page;
                        this.load_page(p, cx);
                    }
                    Err(e) => d.note = Some((e.to_string(), true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- delete ---------------------------------------------------------------------

    /// Asks first, then deletes `keys` in the background and drops them from the list.
    fn confirm_delete(&mut self, keys: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if keys.is_empty() {
            return;
        }
        let this = cx.entity().downgrade();
        let title = match keys.as_slice() {
            [one] => format!("Delete “{one}”?"),
            many => format!("Delete {} keys?", many.len()),
        };
        let text = if keys.len() == 1 {
            format!(
                "Runs DEL {}. The key and its value are removed and cannot be restored.",
                quote_arg(&keys[0])
            )
        } else {
            format!(
                "Runs DEL on the {} checked keys. They and their values are removed and cannot be restored.",
                keys.len()
            )
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let keys = keys.clone();
            alert
                .confirm()
                .title(SharedString::from(title.clone()))
                .description(text.clone())
                .ok_text(if keys.len() == 1 {
                    "Delete key"
                } else {
                    "Delete keys"
                })
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, _, cx| {
                    this.update(cx, |b, cx| b.delete_keys(keys.clone(), cx))
                        .ok();
                    true
                })
        });
    }

    fn delete_keys(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        let (engine, sid) = (self.engine.clone(), self.sid);
        let gone = keys.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    // DEL takes many keys; a few hundred per call keeps each command small.
                    for chunk in keys.chunks(500) {
                        let args: Vec<String> = chunk.iter().map(|k| quote_arg(k)).collect();
                        engine.driver_command(sid, &format!("DEL {}", args.join(" ")))?;
                    }
                    Ok::<_, thaan_core::Error>(())
                })
                .await;
            this.update(cx, |this, cx| {
                let gone: HashSet<&String> = gone.iter().collect();
                match res {
                    Ok(()) => {
                        let before = this.keys.len();
                        this.keys.retain(|k| !gone.contains(&k.name));
                        let removed = (before - this.keys.len()) as u64;
                        this.total = this.total.map(|t| t.saturating_sub(removed));
                        this.checked.retain(|k| !gone.contains(k));
                        if this.detail.as_ref().is_some_and(|d| gone.contains(&d.key)) {
                            this.detail = None;
                            this.grid.update(cx, |g, cx| {
                                g.delegate_mut().set(ResultSet::default());
                                g.refresh(cx);
                            });
                        }
                        this.rebuild();
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- add key --------------------------------------------------------------------

    fn open_add_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_error = None;
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let body = view.update(cx, |b, cx| b.add_key_form(cx));
            let create = view.downgrade();
            dialog.title("Add key").w(px(440.)).child(body).footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("add-key-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("add-key-create")
                            .primary()
                            .label("Add key")
                            .on_click(move |_, window, cx| {
                                create.update(cx, |b, cx| b.create_key(window, cx)).ok();
                            }),
                    ),
            )
        });
    }

    fn add_key_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ty = NEW_TYPES[self.new_type];
        let label = |t: &'static str| div().text_xs().text_color(theme.muted_foreground).child(t);
        let type_btns = h_flex()
            .gap_1()
            .children(NEW_TYPES.iter().enumerate().map(|(i, t)| {
                Button::new(SharedString::from(format!("new-type-{t}")))
                    .label(*t)
                    .ghost()
                    .xsmall()
                    .selected(self.new_type == i)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_type = i;
                        cx.notify();
                    }))
            }));
        let (field_label, value_label) = match ty {
            "hash" => (Some("Field"), "Value"),
            "zset" => (Some("Score"), "Member"),
            "list" | "set" => (None, "First item"),
            _ => (None, "Value"),
        };
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&self.new_name).small()),
            )
            .child(v_flex().gap_1().child(label("Type")).child(type_btns))
            .when_some(field_label, |d, l| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(label(l))
                        .child(Input::new(&self.new_field).small()),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(label(value_label))
                    .child(Input::new(&self.new_value).small()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Expires after"))
                    .child(Input::new(&self.new_ttl).small()),
            )
            .when_some(self.new_error.clone(), |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .into_any_element()
    }

    /// Creates the key from the dialog's fields. Refuses a name that already exists.
    fn create_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_string();
        let value = self.new_value.read(cx).value().to_string();
        let field = self.new_field.read(cx).value().to_string();
        let ttl = self.new_ttl.read(cx).value().trim().to_string();
        let ty = NEW_TYPES[self.new_type];
        let fail = |this: &mut Self, msg: &str, cx: &mut Context<Self>| {
            this.new_error = Some(msg.to_string());
            cx.notify();
        };
        if name.is_empty() {
            return fail(self, "Type a key name", cx);
        }
        if !ttl.is_empty() && ttl.parse::<u64>().map_or(true, |t| t == 0) {
            return fail(self, "Expires after takes a whole number of seconds", cx);
        }
        let k = quote_arg(&name);
        let create = match ty {
            "hash" if field.is_empty() => return fail(self, "A hash needs a first field", cx),
            "hash" => format!("HSET {k} {} {}", quote_arg(&field), quote_arg(&value)),
            "zset" if field.trim().parse::<f64>().is_err() => {
                return fail(self, "Score must be a number", cx)
            }
            "zset" => format!("ZADD {k} {} {}", field.trim(), quote_arg(&value)),
            "list" => format!("RPUSH {k} {}", quote_arg(&value)),
            "set" => format!("SADD {k} {}", quote_arg(&value)),
            _ => format!("SET {k} {}", quote_arg(&value)),
        };
        let (engine, sid) = (self.engine.clone(), self.sid);
        let ttl_cmd = (!ttl.is_empty()).then(|| format!("EXPIRE {k} {ttl}"));
        let exists = format!("EXISTS {k}");
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let n = engine.driver_command(sid, &exists)?;
                    if matches!(n.rows.first().and_then(|r| r.first()), Some(thaan_core::Value::Int(i)) if *i > 0) {
                        return Ok(false);
                    }
                    engine.driver_command(sid, &create)?;
                    if let Some(t) = ttl_cmd {
                        engine.driver_command(sid, &t)?;
                    }
                    Ok::<_, thaan_core::Error>(true)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(true) => {
                        window.close_dialog(cx);
                        for s in [&this.new_name, &this.new_value, &this.new_field, &this.new_ttl] {
                            s.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        this.keys.push(KeyEntry {
                            name: name.clone(),
                            key_type: ty.to_string(),
                            ttl: ttl.parse().ok(),
                        });
                        this.total = this.total.map(|t| t + 1);
                        // Open its folders so the new key is in view, and pick it.
                        let parts: Vec<&str> = name.split(SEP).collect();
                        for i in 1..parts.len() {
                            this.open.insert(parts[..i].join(&SEP.to_string()));
                        }
                        this.rebuild();
                        let ix = this.keys.len() - 1;
                        this.select(ix, cx);
                    }
                    Ok(false) => this.new_error = Some(format!("“{name}” already exists")),
                    Err(e) => this.new_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- render ---------------------------------------------------------------------

    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = match self.total {
            Some(t) if self.keys.len() as u64 != t => {
                format!("{} / {t} keys loaded", self.keys.len())
            }
            _ => format!("{} keys", self.keys.len()),
        };
        let toggle = |id: &'static str, ic: &'static str, label: &'static str, on: bool| {
            Button::new(id)
                .icon(icon(ic))
                .label(label)
                .ghost()
                .small()
                .selected(on)
        };
        let header =
            v_flex()
                .gap_2()
                .p_3()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(if self.loading {
                                    format!("{count} · loading…")
                                } else {
                                    count
                                }),
                        )
                        .child(
                            Button::new("select-all")
                                .label(if self.checked.is_empty() {
                                    "Select all"
                                } else {
                                    "Clear"
                                })
                                .ghost()
                                .xsmall()
                                .disabled(self.keys.is_empty() || self.read_only)
                                .on_click(cx.listener(|this, _, _, cx| this.select_all(cx))),
                        )
                        .child(
                            Button::new("rescan")
                                .icon(icon("refresh-cw"))
                                .ghost()
                                .xsmall()
                                .tooltip("Scan again")
                                .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                        )
                        .when(!self.read_only, |h| {
                            h.child(
                                Button::new("add-key")
                                    .icon(icon("plus"))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Add key")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_add_key(window, cx)
                                    })),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.pattern)
                                    .small()
                                    .prefix(icon("search").size_3p5()),
                            ),
                        )
                        .child(toggle("fuzzy", "asterisk", "Fuzzy", self.fuzzy).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.fuzzy = !this.fuzzy;
                                this.rescan(cx);
                            }),
                        ))
                        .child(
                            toggle("no-expiry", "clock", "No expiry", self.no_expiry).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.no_expiry = !this.no_expiry;
                                    this.rebuild();
                                    cx.notify();
                                }),
                            ),
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
                        .when(!self.checked.is_empty(), |h| {
                            let n = self.checked.len();
                            h.child(
                                Button::new("delete-checked")
                                    .icon(icon("trash"))
                                    .label(format!("Delete {n}"))
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let mut keys: Vec<String> =
                                            this.checked.iter().cloned().collect();
                                        keys.sort();
                                        this.confirm_delete(keys, window, cx)
                                    })),
                            )
                        }),
                );
        let list = if self.rows.is_empty() {
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
            let picked = self.detail.as_ref().map(|d| d.key.clone());
            let read_only = self.read_only;
            uniform_list("keys", self.rows.len(), move |range, _, cx| {
                let Some(view) = view.upgrade() else {
                    return Vec::new();
                };
                let b = view.read(cx);
                let theme = cx.theme();
                range
                    .filter_map(|i| {
                        let row = b.rows.get(i)?.clone();
                        let v = view.downgrade();
                        let base = h_flex()
                            .id(("tree", i))
                            .w_full()
                            .h(px(ROW_H))
                            .pr_3()
                            .gap_1p5()
                            .items_center()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.foreground.opacity(0.04)));
                        let check =
                            |id: SharedString,
                             on: bool,
                             ixs: Vec<usize>,
                             v: WeakEntity<KeyBrowser>| {
                                Checkbox::new(id).checked(on).on_click(move |_, _, cx| {
                                    v.update(cx, |b, cx| b.toggle_checked(&ixs, cx)).ok();
                                })
                            };
                        Some(match row {
                            TreeRow::Folder {
                                path,
                                name,
                                depth,
                                keys,
                                open,
                            } => {
                                let all = keys.iter().all(|ix| {
                                    b.keys.get(*ix).is_some_and(|k| b.checked.contains(&k.name))
                                });
                                let p = path.clone();
                                base.pl(px(12. + depth as f32 * INDENT))
                                    .when(!read_only, |d| {
                                        d.child(check(
                                            format!("chk-f-{path}").into(),
                                            all,
                                            keys.clone(),
                                            v.clone(),
                                        ))
                                    })
                                    .child(
                                        icon(if open {
                                            "chevron-down"
                                        } else {
                                            "chevron-right"
                                        })
                                        .size_3p5()
                                        .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        icon(if open { "folder-open" } else { "folder" })
                                            .size_3p5()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(theme.mono_font_family.clone())
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .font_family(theme.mono_font_family.clone())
                                            .text_color(theme.muted_foreground)
                                            .child(keys.len().to_string()),
                                    )
                                    .on_click(move |_, _, cx| {
                                        v.update(cx, |b, cx| b.toggle_folder(p.clone(), cx)).ok();
                                    })
                            }
                            TreeRow::Key { ix, label, depth } => {
                                let k = b.keys.get(ix)?;
                                let on = picked.as_deref() == Some(k.name.as_str());
                                let checked = b.checked.contains(&k.name);
                                let ttl_color = if k.ttl.is_some() {
                                    theme.warning
                                } else {
                                    theme.muted_foreground
                                };
                                // Keys sit under their folder's label, past the chevron.
                                base.pl(px(12.
                                    + depth as f32 * INDENT
                                    + if depth > 0 { 20. } else { 0. }))
                                    .when(on, |d| d.bg(theme.foreground.opacity(0.06)))
                                    .when(!read_only, |d| {
                                        d.child(check(
                                            format!("chk-k-{ix}").into(),
                                            checked,
                                            vec![ix],
                                            v.clone(),
                                        ))
                                    })
                                    .child(
                                        icon("key-round")
                                            .size_3p5()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(theme.mono_font_family.clone())
                                            .child(if label.is_empty() {
                                                "(empty)".to_string()
                                            } else {
                                                label
                                            }),
                                    )
                                    .child(badge(
                                        k.key_type.clone(),
                                        type_color(theme, &k.key_type),
                                    ))
                                    .child(badge(ttl_text(k.ttl), ttl_color))
                                    .on_click(move |_, _, cx| {
                                        v.update(cx, |b, cx| b.select(ix, cx)).ok();
                                    })
                            }
                        })
                    })
                    .collect()
            })
            .track_scroll(&self.list)
            .flex_1()
            .into_any_element()
        };
        let capped = (self.keys.len() >= KEY_CAP).then(|| {
            div()
                .p_3()
                .border_t_1()
                .border_color(theme.border)
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
        let ttl_color = if d.ttl.is_some() {
            theme.warning
        } else {
            theme.muted_foreground
        };
        let header = h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .font_family(theme.mono_font_family.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .child(d.key.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(badge(d.key_type.clone(), type_color(&theme, &d.key_type)))
                            .child(badge(format!("size {}", d.total), theme.muted_foreground))
                            .child(badge(ttl_text(d.ttl), ttl_color)),
                    ),
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
            .child(
                Button::new("copy-key")
                    .icon(icon("copy"))
                    .ghost()
                    .small()
                    .tooltip("Copy key name")
                    .on_click({
                        let k = d.key.clone();
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(k.clone()))
                    }),
            )
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("ttl-key")
                        .icon(icon("clock"))
                        .ghost()
                        .small()
                        .tooltip("Set expiry")
                        .on_click(cx.listener(|this, _, window, cx| this.open_ttl(window, cx))),
                )
            })
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("rename-key")
                        .icon(icon("pencil"))
                        .ghost()
                        .small()
                        .tooltip("Rename key")
                        .on_click(cx.listener(|this, _, window, cx| this.open_rename(window, cx))),
                )
            })
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("delete-key")
                        .icon(icon("trash"))
                        .ghost()
                        .small()
                        .tooltip("Delete key")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(d) = this.detail.as_ref() {
                                let k = vec![d.key.clone()];
                                this.confirm_delete(k, window, cx)
                            }
                        })),
                )
            });
        let searchable = matches!(d.key_type.as_str(), "hash" | "set" | "zset");
        let addable =
            !self.read_only && matches!(d.key_type.as_str(), "list" | "hash" | "set" | "zset");
        let two = matches!(d.key_type.as_str(), "hash" | "zset");
        let unit = match d.key_type.as_str() {
            "hash" => "fields",
            "list" => "items",
            "set" | "zset" => "members",
            "stream" => "entries",
            _ => "value",
        };
        let tools = (searchable || addable).then(|| {
            h_flex()
                .flex_none()
                .h(px(44.))
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .when(searchable, |h| {
                    h.child(
                        div().w(px(180.)).flex_none().child(
                            Input::new(&self.item_search)
                                .small()
                                .prefix(icon("search").size_3p5()),
                        ),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(format!("{} {unit}", d.total)),
                )
                .child(div().flex_1())
                .when(addable, |h| {
                    h.child(
                        div()
                            .flex_1()
                            .min_w(px(72.))
                            .max_w(px(if two { 140. } else { 220. }))
                            .child(Input::new(&self.item_a).small()),
                    )
                    .when(two, |h| {
                        h.child(
                            div()
                                .flex_1()
                                .min_w(px(72.))
                                .max_w(px(180.))
                                .child(Input::new(&self.item_b).small()),
                        )
                    })
                    .when(d.key_type == "list", |h| {
                        h.child(
                            Button::new("push-head")
                                .label("Head")
                                .ghost()
                                .xsmall()
                                .selected(self.push_head)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.push_head = true;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("push-tail")
                                .label("Tail")
                                .ghost()
                                .xsmall()
                                .selected(!self.push_head)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.push_head = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("add-item")
                            .icon(icon("plus"))
                            .label(match d.key_type.as_str() {
                                "list" => "Push",
                                "hash" => "Add field",
                                _ => "Add member",
                            })
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_item(window, cx))),
                    )
                })
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
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .child(match (&d.error, d.loading, &d.note) {
                        (Some(e), _, _) => div().text_color(theme.danger).child(e.clone()),
                        (None, true, _) => div().child(format!("Loading {PAGE} rows…")),
                        (None, false, Some((n, true))) => {
                            div().text_color(theme.danger).child(n.clone())
                        }
                        (None, false, Some((n, false))) => {
                            div().child(format!("{n} · page {} of {pages}", d.page + 1))
                        }
                        (None, false, None) => div().child(format!(
                            "Page {} of {pages} · double-click a row to view or edit it",
                            d.page + 1
                        )),
                    }),
            )
            .child(
                Button::new("open-item")
                    .icon(icon("eye"))
                    .label("Open item")
                    .ghost()
                    .xsmall()
                    .disabled(self.picked_row.is_none() || d.loading)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(r) = this.picked_row {
                            this.open_item(r, window, cx);
                        }
                    })),
            )
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
            .children(tools)
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

/// How the item dialog shows a value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueView {
    Text,
    Json,
    Hex,
    Base64,
}

const VIEWS: [(ValueView, &str); 4] = [
    (ValueView::Text, "Text"),
    (ValueView::Json, "JSON"),
    (ValueView::Hex, "Hex"),
    (ValueView::Base64, "Base64"),
];

/// The item open in the dialog: one row of the key's grid.
struct ItemView {
    key: String,
    /// `#3`, a hash field, `member`…
    title: String,
    /// Extra facts for the header (`score 120`).
    meta: Option<String>,
    /// The row as the grid had it; identifies the item when saving.
    row: Vec<(String, thaan_core::Value)>,
    /// The column that holds the value shown (`value`, `member`, `fields`).
    column: &'static str,
    bytes: Vec<u8>,
    decoder: Decoder,
    /// `bytes` after `decoder`, or why it failed.
    shown: Result<Vec<u8>, String>,
    view: ValueView,
    wrap: bool,
    editing: bool,
    saving: bool,
    error: Option<String>,
}

fn value_bytes(v: &thaan_core::Value) -> Vec<u8> {
    match v {
        thaan_core::Value::Null => Vec::new(),
        thaan_core::Value::Bytes(b) => b.clone(),
        thaan_core::Value::Text(t) => t.clone().into_bytes(),
        other => other.to_string().into_bytes(),
    }
}

/// Standard base64 with padding (RFC 4648).
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Bytes as hex, 16 to a line with their offset.
fn hex_dump(b: &[u8]) -> String {
    b.chunks(16)
        .enumerate()
        .map(|(i, c)| {
            let hex: Vec<String> = c.iter().map(|x| format!("{x:02x}")).collect();
            format!("{:08x}  {}", i * 16, hex.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_pretty(b: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(b).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

impl ItemView {
    /// The decoded bytes (the raw ones while the decoder fails).
    fn data(&self) -> &[u8] {
        self.shown.as_deref().unwrap_or(&self.bytes)
    }

    fn text(&self) -> String {
        let b = self.data();
        match self.view {
            ValueView::Text => String::from_utf8_lossy(b).into_owned(),
            ValueView::Json => {
                json_pretty(b).unwrap_or_else(|| String::from_utf8_lossy(b).into_owned())
            }
            ValueView::Hex => hex_dump(b),
            ValueView::Base64 => base64(b),
        }
    }

    fn utf8(&self) -> bool {
        std::str::from_utf8(self.data()).is_ok()
    }

    fn set_decoder(&mut self, d: Decoder) {
        self.decoder = d;
        self.shown = decode(d, &self.bytes);
        // Structured decoders produce JSON: show it as such.
        if self.shown.is_ok() && matches!(d, Decoder::Msgpack | Decoder::Pickle | Decoder::Protobuf)
        {
            self.view = ValueView::Json;
        }
    }
}

impl KeyBrowser {
    // ---- rename ---------------------------------------------------------------------

    fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_ref() else {
            return;
        };
        let current = d.key.clone();
        self.rename_error = None;
        self.rename_input
            .update(cx, |s, cx| s.set_value(current, window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let body = view.update(cx, |b, cx| {
                let theme = cx.theme().clone();
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Key name"),
                    )
                    .child(Input::new(&b.rename_input).small())
                    .when_some(b.rename_error.clone(), |d, e| {
                        d.child(div().pt_1().text_sm().text_color(theme.danger).child(e))
                    })
                    .into_any_element()
            });
            let go = view.downgrade();
            dialog.title("Rename key").w(px(440.)).child(body).footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("rename-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rename-ok")
                            .icon(icon("pencil"))
                            .primary()
                            .label("Rename key")
                            .on_click(move |_, window, cx| {
                                go.update(cx, |b, cx| b.rename(window, cx)).ok();
                            }),
                    ),
            )
        });
    }

    /// `RENAMENX`: never overwrites a key that already has the new name.
    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = self.detail.as_ref().map(|d| d.key.clone()) else {
            return;
        };
        let new = self.rename_input.read(cx).value().trim().to_string();
        if new.is_empty() {
            self.rename_error = Some("Type a name".into());
            cx.notify();
            return;
        }
        if new == old {
            window.close_dialog(cx);
            return;
        }
        let (engine, sid) = (self.engine.clone(), self.sid);
        let cmd = format!("RENAMENX {} {}", quote_arg(&old), quote_arg(&new));
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.driver_command(sid, &cmd) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(rs)
                        if rs.rows.first().and_then(|r| r.first())
                            == Some(&thaan_core::Value::Int(1)) =>
                    {
                        window.close_dialog(cx);
                        if let Some(k) = this.keys.iter_mut().find(|k| k.name == old) {
                            k.name = new.clone();
                        }
                        if this.checked.remove(&old) {
                            this.checked.insert(new.clone());
                        }
                        if let Some(d) = this.detail.as_mut() {
                            d.key = new.clone();
                        }
                        let parts: Vec<&str> = new.split(SEP).collect();
                        for i in 1..parts.len() {
                            this.open.insert(parts[..i].join(&SEP.to_string()));
                        }
                        this.rebuild();
                    }
                    Ok(_) => this.rename_error = Some(format!("“{new}” already exists")),
                    Err(e) => this.rename_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- item dialog ----------------------------------------------------------------

    fn open_item(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_ref() else {
            return;
        };
        let fields = self.grid.read(cx).delegate().row_fields(row);
        let get = |c: &str| fields.iter().find(|(n, _)| n == c).map(|(_, v)| v.clone());
        let (title, column, meta) = match d.key_type.as_str() {
            "hash" => (
                get("field").map(|v| v.to_string()).unwrap_or_default(),
                "value",
                None,
            ),
            "list" => (
                format!(
                    "#{}",
                    get("index").map(|v| v.to_string()).unwrap_or_default()
                ),
                "value",
                None,
            ),
            "set" => ("Member".to_string(), "member", None),
            "zset" => (
                "Member".to_string(),
                "member",
                get("score").map(|s| format!("score {s}")),
            ),
            "stream" => (
                get("id").map(|v| v.to_string()).unwrap_or_default(),
                "fields",
                None,
            ),
            _ => ("Value".to_string(), "value", None),
        };
        let bytes = get(column).map(|v| value_bytes(&v)).unwrap_or_default();
        // JSON reads best indented; binary as hex; the rest as text.
        let view = if json_pretty(&bytes).is_some() {
            ValueView::Json
        } else if std::str::from_utf8(&bytes).is_err() {
            ValueView::Hex
        } else {
            ValueView::Text
        };
        self.item = Some(ItemView {
            key: d.key.clone(),
            title,
            meta,
            row: fields,
            column,
            shown: Ok(bytes.clone()),
            bytes,
            decoder: Decoder::None,
            view,
            wrap: true,
            editing: false,
            saving: false,
            error: None,
        });
        let entity = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (title, body, footer) = entity.update(cx, |b, cx| b.item_parts(cx));
            let gone = entity.downgrade();
            dialog
                .title(title)
                .w(px(760.))
                .child(body)
                .footer(footer)
                .on_close(move |_, _, cx| {
                    gone.update(cx, |b, cx| {
                        b.item = None;
                        cx.notify();
                    })
                    .ok();
                })
        });
    }

    /// Title, body and footer of the item dialog, from the current state (the dialog
    /// builder runs every frame).
    fn item_parts(&mut self, cx: &mut Context<Self>) -> (AnyElement, AnyElement, AnyElement) {
        let theme = cx.theme().clone();
        let Some(it) = self.item.as_ref() else {
            let empty = || div().into_any_element();
            return (empty(), empty(), empty());
        };
        let editable =
            !self.read_only && it.column != "fields" && it.decoder == Decoder::None && it.utf8();
        let title = h_flex()
            .gap_2()
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .child(it.title.clone()),
            )
            .child(badge(
                if it.utf8() { "UTF-8" } else { "binary" },
                theme.muted_foreground,
            ))
            .child(badge(
                format!("{} B", it.bytes.len()),
                theme.muted_foreground,
            ))
            .when_some(it.meta.clone(), |h, m| {
                h.child(badge(m, theme.muted_foreground))
            })
            .into_any_element();
        let view_now = it.view;
        let has_json = json_pretty(&it.bytes).is_some();
        let views = ButtonGroup::new("item-view")
            .outline()
            .xsmall()
            .children(VIEWS.iter().map(|(v, label)| {
                Button::new(SharedString::from(format!("view-{label}")))
                    .label(*label)
                    .selected(view_now == *v)
                    .disabled(*v == ValueView::Json && !has_json)
            }))
            .on_click(cx.listener(|this, clicks: &Vec<usize>, _, cx| {
                if let (Some(it), Some(&i)) = (this.item.as_mut(), clicks.first()) {
                    it.view = VIEWS[i].0;
                    cx.notify();
                }
            }));
        let dec_now = it.decoder;
        let decoders: Vec<Decoder> = Decoder::ALL.into_iter().filter(|d| d.available()).collect();
        let decode_row = h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .w(px(44.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Decode"),
            )
            .child(
                ButtonGroup::new("item-decode")
                    .outline()
                    .xsmall()
                    .children(decoders.iter().map(|d| {
                        Button::new(SharedString::from(format!("dec-{}", d.label())))
                            .label(d.label())
                            .selected(dec_now == *d)
                    }))
                    .on_click(cx.listener(move |this, clicks: &Vec<usize>, _, cx| {
                        if let (Some(it), Some(&i)) = (this.item.as_mut(), clicks.first()) {
                            if let Some(d) = decoders.get(i) {
                                it.set_decoder(*d);
                                cx.notify();
                            }
                        }
                    })),
            );
        let toolbar = h_flex()
            .gap_3()
            .items_center()
            .child(
                div()
                    .w(px(44.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("View"),
            )
            .child(views)
            .child(div().flex_1())
            .child(
                Checkbox::new("item-wrap")
                    .label("Word wrap")
                    .checked(it.wrap)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        if let Some(it) = this.item.as_mut() {
                            it.wrap = *v;
                            cx.notify();
                        }
                    })),
            );
        let content: AnyElement = if it.editing {
            div()
                .h(px(380.))
                .border_1()
                .border_color(theme.input)
                .rounded(px(6.))
                .overflow_hidden()
                .child(Editor::new(&self.item_editor).bordered(false).h_full())
                .into_any_element()
        } else {
            let text = it.text();
            let wrap = it.wrap;
            div()
                .id("item-value")
                .h(px(380.))
                .p_3()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.foreground.opacity(0.03))
                .overflow_scroll()
                .text_sm()
                .font_family(theme.mono_font_family.clone())
                .children(text.lines().map(|l| {
                    let line = if l.is_empty() {
                        " ".to_string()
                    } else {
                        l.to_string()
                    };
                    div().when(!wrap, |d| d.whitespace_nowrap()).child(line)
                }))
                .into_any_element()
        };
        let decode_error = it.shown.as_ref().err().cloned();
        let body = v_flex()
            .gap_3()
            .child(decode_row)
            .child(toolbar)
            .when_some(decode_error, |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .child(content)
            .when_some(it.error.clone(), |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .into_any_element();
        let copy = it.text();
        let footer = if it.editing {
            h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("item-cancel")
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(it) = this.item.as_mut() {
                                it.editing = false;
                                it.error = None;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("item-save")
                        .icon(icon("check"))
                        .primary()
                        .label(if it.saving { "Saving…" } else { "Save" })
                        .disabled(it.saving)
                        .on_click(cx.listener(|this, _, window, cx| this.save_item(window, cx))),
                )
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if editable || self.read_only {
                            ""
                        } else if it.decoder != Decoder::None {
                            "Set Decode to None to edit the stored value"
                        } else if it.column == "fields" {
                            "Stream entries cannot be changed"
                        } else {
                            "Binary values change from the query editor"
                        }),
                )
                .child(
                    Button::new("item-copy")
                        .icon(icon("copy"))
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                )
                .when(editable, |h| {
                    h.child(
                        Button::new("item-edit")
                            .icon(icon("pencil"))
                            .primary()
                            .label("Edit")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this
                                    .item
                                    .as_ref()
                                    .map(|it| String::from_utf8_lossy(&it.bytes).into_owned())
                                    .unwrap_or_default();
                                this.item_editor
                                    .update(cx, |e, cx| e.set_value(text, window, cx));
                                if let Some(it) = this.item.as_mut() {
                                    it.editing = true;
                                }
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
        };
        (title, body, footer)
    }

    /// Saves the edited value as a staged change, so the driver writes it the same way
    /// as a grid edit (in one MULTI / EXEC).
    fn save_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new = self.item_editor.read(cx).value().to_string();
        let Some(it) = self.item.as_mut() else {
            return;
        };
        let key_cols: Vec<(String, thaan_core::Value)> = match it.column {
            // A set or sorted-set member is its own key: the old one is replaced.
            "member" => it
                .row
                .iter()
                .filter(|(c, _)| c == "member")
                .cloned()
                .collect(),
            _ => it
                .row
                .iter()
                .filter(|(c, _)| c == "field" || c == "index")
                .cloned()
                .collect(),
        };
        let change = thaan_core::table::StagedChange::Update {
            key: key_cols,
            set: vec![(it.column.to_string(), thaan_core::Value::Text(new.clone()))],
        };
        it.saving = true;
        it.error = None;
        let key = it.key.clone();
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let k = key.clone();
            let res = cx
                .background_spawn(async move {
                    engine.apply_changes(sid, &ObjectRef::new(k), &[change])
                })
                .await;
            this.update_in(cx, |this, _, cx| {
                if let Some(it) = this.item.as_mut() {
                    it.saving = false;
                    match res {
                        Ok(_) => {
                            it.bytes = new.into_bytes();
                            it.shown = Ok(it.bytes.clone());
                            it.editing = false;
                            // The member is the item's identity: follow the new one.
                            if it.column == "member" {
                                for (c, v) in it.row.iter_mut() {
                                    if c == "member" {
                                        *v = thaan_core::Value::Text(
                                            String::from_utf8_lossy(&it.bytes).into_owned(),
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => it.error = Some(e.to_string()),
                    }
                }
                let page = this.detail.as_ref().map_or(0, |d| d.page);
                if this.detail.as_ref().is_some_and(|d| d.key == key) {
                    this.load_page(page, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The right pane's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Key,
    Console,
}

/// One command run in the Command line tab.
struct CmdEntry {
    command: String,
    /// The reply as text, or the error.
    out: Result<String, String>,
    /// The reply's first line names its columns (drawn dimmer).
    header: bool,
    ms: u64,
}

/// Command-line entries kept per tab.
const CMD_KEEP: usize = 200;
/// Reply rows printed per command; the rest are counted.
const CMD_ROWS: usize = 500;

/// An error the way Redis said it: no `database error:` wrapper, and without the
/// empty `with args beginning with:` Redis appends to unknown commands.
fn error_text(e: &thaan_core::Error) -> String {
    let msg = match e {
        thaan_core::Error::Database(m) => m.clone(),
        other => other.to_string(),
    };
    let msg = msg.trim_end();
    msg.strip_suffix(", with args beginning with:")
        .unwrap_or(msg)
        .to_string()
}

/// A reply as aligned text columns, the way a terminal client prints it.
fn result_text(rs: &ResultSet) -> String {
    if rs.columns.len() == 1 && rs.rows.len() == 1 {
        return rs.rows[0][0].to_string();
    }
    let rows: Vec<Vec<String>> = rs
        .rows
        .iter()
        .take(CMD_ROWS)
        .map(|r| r.iter().map(|v| v.to_string().replace('\n', "⏎")).collect())
        .collect();
    let mut width: Vec<usize> = rs.columns.iter().map(|c| c.name.chars().count()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            if let Some(w) = width.get_mut(i) {
                *w = (*w).max(c.chars().count()).min(60);
            }
        }
    }
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = width.get(i).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![line(rs.columns.iter().map(|c| c.name.clone()).collect())];
    out.extend(rows.into_iter().map(line));
    if rs.rows.len() > CMD_ROWS {
        out.push(format!("… {} more rows", rs.rows.len() - CMD_ROWS));
    }
    if rs.rows.is_empty() {
        out.push("(empty)".into());
    }
    out.join("\n")
}

impl KeyBrowser {
    // ---- expiry ---------------------------------------------------------------------

    fn open_ttl(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_ref() else {
            return;
        };
        let now = d.ttl.map(|t| t.to_string()).unwrap_or_default();
        self.ttl_error = None;
        self.ttl_input
            .update(cx, |s, cx| s.set_value(now, window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let body = view.update(cx, |b, cx| {
                let theme = cx.theme().clone();
                let now = b.detail.as_ref().and_then(|d| d.ttl);
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Expires after (seconds)"),
                    )
                    .child(Input::new(&b.ttl_input).small())
                    .child(
                        div()
                            .pt_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match now {
                                Some(t) => format!("Now: expires in {}", ttl_text(Some(t))),
                                None => "Now: never expires".to_string(),
                            }),
                    )
                    .when_some(b.ttl_error.clone(), |d, e| {
                        d.child(div().pt_1().text_sm().text_color(theme.danger).child(e))
                    })
                    .into_any_element()
            });
            let (save, persist) = (view.downgrade(), view.downgrade());
            dialog.title("Set expiry").w(px(440.)).child(body).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new("ttl-persist")
                            .label("Remove expiry")
                            .ghost()
                            .on_click(move |_, window, cx| {
                                persist.update(cx, |b, cx| b.set_ttl(None, window, cx)).ok();
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("ttl-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("ttl-save")
                            .icon(icon("clock"))
                            .primary()
                            .label("Set expiry")
                            .on_click(move |_, window, cx| {
                                save.update(cx, |b, cx| {
                                    let v = b.ttl_input.read(cx).value().trim().to_string();
                                    match v.parse::<u64>() {
                                        Ok(n) if n > 0 => b.set_ttl(Some(n), window, cx),
                                        _ => {
                                            b.ttl_error =
                                                Some("Type a whole number of seconds".into());
                                            cx.notify();
                                        }
                                    }
                                })
                                .ok();
                            }),
                    ),
            )
        });
    }

    /// `EXPIRE key n`, or `PERSIST key` for `None`.
    fn set_ttl(&mut self, secs: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.detail.as_ref().map(|d| d.key.clone()) else {
            return;
        };
        let cmd = match secs {
            Some(n) => format!("EXPIRE {} {n}", quote_arg(&key)),
            None => format!("PERSIST {}", quote_arg(&key)),
        };
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.driver_command(sid, &cmd) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(_) => {
                        window.close_dialog(cx);
                        let ttl = secs.map(|n| n as i64);
                        if let Some(d) = this.detail.as_mut().filter(|d| d.key == key) {
                            d.ttl = ttl;
                        }
                        if let Some(k) = this.keys.iter_mut().find(|k| k.name == key) {
                            k.ttl = ttl;
                        }
                        this.rebuild();
                    }
                    Err(e) => this.ttl_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- command line ---------------------------------------------------------------

    fn run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.cmd_input.read(cx).value().trim().to_string();
        if command.is_empty() || self.cmd_busy {
            return;
        }
        self.cmd_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.cmd_busy = true;
        cx.notify();
        let (engine, sid) = (self.engine.clone(), self.sid);
        let c = command.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.driver_command(sid, &c) })
                .await;
            this.update(cx, |this, cx| {
                this.cmd_busy = false;
                let (out, header, ms) = match res {
                    Ok(r) => {
                        let header = !(r.columns.len() == 1 && r.rows.len() == 1);
                        (Ok(result_text(&r)), header, r.elapsed_ms)
                    }
                    Err(e) => (Err(error_text(&e)), false, 0),
                };
                this.cmd_log.push_back(CmdEntry {
                    command,
                    out,
                    header,
                    ms,
                });
                while this.cmd_log.len() > CMD_KEEP {
                    this.cmd_log.pop_front();
                }
                this.cmd_scroll.scroll_to_bottom();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts a past command back in the prompt.
    fn recall(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self.cmd_log.get(ix) {
            let c = e.command.clone();
            self.cmd_input.update(cx, |s, cx| {
                s.set_value(c, window, cx);
                s.focus(window, cx);
            });
        }
    }

    fn render_console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let prompt = format!("{}>", self.db);
        let entries = self.cmd_log.iter().enumerate().map(|(ix, e)| {
            let reply = match &e.out {
                Ok(t) => v_flex().children(t.lines().enumerate().map(|(i, l)| {
                    div()
                        .whitespace_nowrap()
                        .text_color(if e.header && i == 0 {
                            theme.muted_foreground
                        } else {
                            theme.foreground
                        })
                        .child(l.to_string())
                })),
                Err(err) => v_flex().child(div().text_color(theme.danger).child(err.clone())),
            };
            v_flex()
                .gap_1()
                .pb_4()
                .child(
                    h_flex()
                        .id(("cmd", ix))
                        .gap_2()
                        .cursor_pointer()
                        .rounded(px(4.))
                        .hover(|d| d.bg(theme.foreground.opacity(0.04)))
                        .child(div().text_color(theme.primary).child(prompt.clone()))
                        .child(div().flex_1().child(e.command.clone()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            if e.ms == 0 {
                                "<1 ms".to_string()
                            } else {
                                format!("{} ms", e.ms)
                            },
                        ))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.recall(ix, window, cx)),
                        ),
                )
                .child(reply)
        });
        let busy = self
            .cmd_busy
            .then(|| div().text_color(theme.muted_foreground).child("Running…"));
        v_flex()
            .size_full()
            .child(
                div()
                    .id("cmd-log")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .overflow_scroll()
                    .track_scroll(&self.cmd_scroll)
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .child(
                        div()
                            .pb_4()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "Redis on {}. Type a command and press Enter; click a past one to run it again",
                                self.db
                            )),
                    )
                    .children(entries)
                    .children(busy),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h(px(44.))
                    .px_4()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .child(div().text_color(theme.primary).child(prompt))
                    .child(
                        div().flex_1().child(
                            Input::new(&self.cmd_input)
                                .appearance(false)
                                .font_family(theme.mono_font_family.clone()),
                        ),
                    ),
            )
    }

    fn render_right_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let now = self.right_tab;
        h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                ButtonGroup::new("right-tabs")
                    .outline()
                    .small()
                    .child(
                        Button::new("tab-key")
                            .icon(icon("key-round"))
                            .label("Key")
                            .selected(now == RightTab::Key),
                    )
                    .child(
                        Button::new("tab-cmd")
                            .icon(icon("square-terminal"))
                            .label("Command line")
                            .selected(now == RightTab::Console),
                    )
                    .on_click(cx.listener(|this, clicks: &Vec<usize>, _, cx| {
                        this.right_tab = match clicks.first() {
                            Some(1) => RightTab::Console,
                            _ => RightTab::Key,
                        };
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .when(now == RightTab::Console, |h| {
                h.child(
                    Button::new("cmd-clear")
                        .icon(icon("trash"))
                        .ghost()
                        .small()
                        .tooltip("Clear the output")
                        .disabled(self.cmd_log.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cmd_log.clear();
                            cx.notify();
                        })),
                )
            })
    }
}

impl Render for KeyBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((a, b)) = self.placeholders.take() {
            self.item_a
                .update(cx, |s, cx| s.set_placeholder(a, window, cx));
            self.item_b
                .update(cx, |s, cx| s.set_placeholder(b, window, cx));
        }
        let border = cx.theme().border;
        h_flex()
            .size_full()
            .child(
                div()
                    .w(px(400.))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(border)
                    .child(self.render_list(cx)),
            )
            .child({
                let tabs = self.render_right_tabs(cx);
                let body = match self.right_tab {
                    RightTab::Key => self.render_detail(cx).into_any_element(),
                    RightTab::Console => self.render_console(cx).into_any_element(),
                };
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(tabs)
                    .child(div().flex_1().min_h_0().child(body))
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` attribute in through `gpui_kit::*`.
    use super::{base64, build_tree, error_text, hex_dump, result_text, ttl_text, TreeRow};
    use std::collections::BTreeSet;
    use thaan_core::{Column, ResultSet, Value};

    #[test]
    fn ttls_read_short() {
        assert_eq!(ttl_text(None), "no expiry");
        assert_eq!(ttl_text(Some(45)), "45 s");
        assert_eq!(ttl_text(Some(600)), "10 min");
        assert_eq!(ttl_text(Some(7_300)), "2 h");
        assert_eq!(ttl_text(Some(200_000)), "2 d");
    }

    fn labels(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Folder {
                    name, depth, keys, ..
                } => format!("{depth}:{name}/({})", keys.len()),
                TreeRow::Key { label, depth, .. } => format!("{depth}:{label}"),
            })
            .collect()
    }

    #[test]
    fn keys_group_by_colon() {
        let names = [
            "user:2",
            "greeting",
            "user:1",
            "cache:item:1",
            "cache:item:2",
            "jobs:queue",
        ];
        let closed = build_tree(&names, |_| true, &BTreeSet::new());
        assert_eq!(
            labels(&closed),
            ["0:cache/(2)", "0:jobs/(1)", "0:user/(2)", "0:greeting"]
        );

        let open: BTreeSet<String> = ["cache", "cache:item", "user"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = build_tree(&names, |_| true, &open);
        assert_eq!(
            labels(&rows),
            [
                "0:cache/(2)",
                "1:item/(2)",
                "2:1",
                "2:2",
                "0:jobs/(1)",
                "0:user/(2)",
                "1:1",
                "1:2",
                "0:greeting"
            ]
        );
        // Filtered keys leave their folder's count too.
        let rows = build_tree(&names, |ix| names[ix] != "user:1", &open);
        assert!(labels(&rows).contains(&"0:user/(1)".to_string()));
    }

    #[test]
    fn value_views() {
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(hex_dump(b"AB"), "00000000  41 42");
    }

    #[test]
    fn command_output_lines_up() {
        let rs = ResultSet {
            columns: vec![Column::new("field"), Column::new("value")],
            rows: vec![
                vec![Value::from("name"), Value::from("Ann")],
                vec![Value::from("city"), Value::from("Chiang Mai")],
            ],
            ..Default::default()
        };
        assert_eq!(
            result_text(&rs),
            "field  value\nname   Ann\ncity   Chiang Mai"
        );
        let one = ResultSet {
            columns: vec![Column::new("reply")],
            rows: vec![vec![Value::from("OK")]],
            ..Default::default()
        };
        assert_eq!(result_text(&one), "OK");
    }

    #[test]
    fn errors_read_like_redis() {
        let e = thaan_core::Error::Database(
            "ERR unknown command 'foo', with args beginning with: ".into(),
        );
        assert_eq!(error_text(&e), "ERR unknown command 'foo'");
        let e = thaan_core::Error::Database("WRONGTYPE Operation against a key".into());
        assert_eq!(error_text(&e), "WRONGTYPE Operation against a key");
    }
}
