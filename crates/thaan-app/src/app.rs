//! Main window. All work is dispatched to `thaan_core::Engine`; this file only holds view
//! state and layout (see AGENTS.md, principle 1 and 3).

#[path = "app_link.rs"]
mod app_link;
#[path = "cell_edit.rs"]
mod cell_edit;
#[path = "command_bar.rs"]
mod command_bar;
#[path = "doc_editor.rs"]
mod doc_editor;
#[path = "live.rs"]
mod live;
#[path = "settings.rs"]
mod settings;
#[path = "sidebar_tree.rs"]
mod sidebar_tree;
#[path = "structure.rs"]
mod structure;
#[path = "table_ops.rs"]
mod table_ops;
#[path = "transfer_dialog.rs"]
mod transfer_dialog;
#[path = "views.rs"]
mod views;

use sidebar_tree::ObjectTree;
#[path = "home.rs"]
mod home;

use crate::docview::{view_switch, DocPanel, DocView};
use crate::grid::{track_focus, ResultGrid, GRID_HEADER_H};
use crate::theme;
use gpui_kit::base::{Disableable as _, Selectable as _, StyledExt as _};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{ColumnSort, DataTable, TableEvent, TableState};
use gpui_kit::component::IndexPath;
use gpui_kit::component::{
    h_flex, h_resizable, resizable_panel, v_flex, v_resizable, ActiveTheme as _, Icon, Sizable as _,
};
use gpui_kit::component::{TitleBar, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use thaan_core::commands::{builtin_commands, CommandSpec};
use thaan_core::connection::ConnectionConfig;
use thaan_core::driver::{DriverInfo, ParamKind};
use thaan_core::engine::{Engine, QueryRequest, RunScope, SessionInfo, StatementResult, TablePage};
use thaan_core::export::{ExportFormat, ExportOptions};
use thaan_core::schema::{ObjectKind, ObjectRef, SchemaObject, TableInfo};
use thaan_core::store::{HistoryEntry, SavedQuery};
use thaan_core::table::{Filter, FilterOp, PageRequest, Sort};
use thaan_core::ResultSet;

actions!(
    thaan,
    [
        RunCurrent,
        RunAll,
        NewTab,
        CloseTab,
        TogglePalette,
        ToggleTheme,
        OpenSettings,
        RefreshSchema,
        Disconnect,
        CopySelection,
        AddRow,
        PasteCell,
        CloneRow,
        DeleteRow
    ]
);

#[cfg(target_os = "macos")]
macro_rules! m {
    ($k:literal) => {
        concat!("cmd-", $k)
    };
}
#[cfg(not(target_os = "macos"))]
macro_rules! m {
    ($k:literal) => {
        concat!("ctrl-", $k)
    };
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(m!("enter"), RunCurrent, Some("ThaanApp")),
        KeyBinding::new(m!("shift-enter"), RunAll, Some("ThaanApp")),
        KeyBinding::new(m!("t"), NewTab, Some("ThaanApp")),
        KeyBinding::new(m!("w"), CloseTab, Some("ThaanApp")),
        KeyBinding::new(m!("k"), TogglePalette, Some("ThaanApp")),
        KeyBinding::new(m!(","), OpenSettings, Some("ThaanApp")),
        KeyBinding::new(m!("r"), RefreshSchema, Some("ThaanApp")),
        // ⌘C in a result grid: the selected row (as TSV for spreadsheets) or cell.
        KeyBinding::new(m!("c"), CopySelection, Some("DataTable")),
        // In a table grid: paste into the selected cell, clone or delete its row.
        KeyBinding::new(m!("v"), PasteCell, Some("DataTable")),
        KeyBinding::new(m!("d"), CloneRow, Some("DataTable")),
        KeyBinding::new("backspace", DeleteRow, Some("DataTable")),
        KeyBinding::new("delete", DeleteRow, Some("DataTable")),
        // ⌘N in a table tab: a new row (a new document on MongoDB).
        KeyBinding::new(m!("n"), AddRow, Some("ThaanApp")),
    ]);
}

fn icon(name: &str) -> Icon {
    Icon::empty().path(SharedString::from(format!("icons/{name}.svg")))
}

/// A database's own logo (Simple Icons, CC0; see `assets.rs`) in one colour, or a lucide
/// fallback for drivers we have no logo for.
fn db_icon(driver_id: &str) -> Icon {
    match crate::assets::db_logo_path(driver_id) {
        Some(path) => Icon::empty().path(path),
        None => icon("database"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Rail {
    Objects,
    Saved,
    History,
}

#[derive(Clone, Copy, PartialEq)]
enum TableView {
    Data,
    Structure,
}

struct QueryTab {
    /// The connection this tab belongs to (tabs of every connection share one strip).
    ws: thaan_core::engine::SessionId,
    /// The session it runs on: the connection's own, or one opened on another database
    /// of the same server (`database` is then set).
    sid: thaan_core::engine::SessionId,
    database: Option<String>,
    title: SharedString,
    saved_id: Option<String>,
    editor: Entity<EditorState>,
    results: Vec<StatementResult>,
    active_result: usize,
    grid: Entity<TableState<ResultGrid>>,
    /// Tree / JSON views of document results (MongoDB).
    docs: Entity<DocPanel>,
    running: bool,
}

type Choice = Entity<SelectState<Vec<SharedString>>>;

/// One column / operator / value row of the filter builder (docs/spec/workspace.md §3).
struct FilterRow {
    id: usize,
    column: Choice,
    op: Choice,
    value: Entity<InputState>,
}

struct TableTab {
    object: ObjectRef,
    /// The connection this tab belongs to (tabs of every connection share one strip).
    ws: thaan_core::engine::SessionId,
    /// The session this tab reads from: the connection's own, or one opened on another
    /// database of the same server (`database` is then set).
    sid: thaan_core::engine::SessionId,
    database: Option<String>,
    info: Option<TableInfo>,
    ddl: Option<String>,
    /// The list the SQL Structure view shows (Columns, Indexes…).
    struct_tab: structure::StructTab,
    /// Raw `WHERE` text (or a document query on MongoDB).
    filter: Entity<InputState>,
    /// Builder rows, combined with AND. Both modes keep their text across a switch.
    filter_rows: Vec<FilterRow>,
    next_filter_id: usize,
    /// `true` while the raw `WHERE` box is shown instead of the builder.
    raw_mode: bool,
    /// Page number box in the status bar (1-based); Enter jumps there.
    page_input: Entity<InputState>,
    /// Rows per page (100 by default); the box in the status bar sets it.
    page_size: u64,
    page_size_input: Entity<InputState>,
    /// When the current page arrived, for the "fetched … ago" note.
    fetched_at: Option<std::time::Instant>,
    page: Option<TablePage>,
    page_ix: u64,
    sort: Vec<Sort>,
    view: TableView,
    grid: Entity<TableState<ResultGrid>>,
    docs: Entity<DocPanel>,
    /// MongoDB: projection and sort documents next to the filter document.
    projection: Entity<InputState>,
    sort_doc: Entity<InputState>,
    /// The command box: opens with a ready-to-run command for this object.
    command: Entity<EditorState>,
    /// The last command's result, shown instead of the paged rows until cleared.
    command_result: Option<StatementResult>,
    command_running: bool,
    /// The command box is shown (it can be hidden to see only the grid).
    command_open: bool,
    error: Option<String>,
    loading: bool,
}

impl TableTab {
    /// The structured filters and raw `WHERE` text for the mode that is showing.
    /// MongoDB projection and sort documents (empty boxes mean none).
    fn document_shape(&self, cx: &App) -> (Option<String>, Option<String>) {
        let text = |i: &Entity<InputState>| {
            let v = i.read(cx).value().trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        (text(&self.projection), text(&self.sort_doc))
    }

    fn where_parts(&self, cx: &App) -> (Vec<Filter>, Option<String>) {
        if self.raw_mode {
            let raw = self.filter.read(cx).value().trim().to_string();
            return (Vec::new(), (!raw.is_empty()).then_some(raw));
        }
        let filters = self
            .filter_rows
            .iter()
            .filter_map(|r| {
                let column = r.column.read(cx).selected_value()?.to_string();
                let op = FilterOp::from_title(r.op.read(cx).selected_value()?)?;
                let data_type = self
                    .info
                    .as_ref()
                    .and_then(|i| i.columns.iter().find(|c| c.name == column))
                    .map(|c| c.data_type.as_str());
                Filter::from_text(&column, op, &r.value.read(cx).value(), data_type)
            })
            .collect();
        (filters, None)
    }
}

enum Tab {
    Query(Box<QueryTab>),
    Table(Box<TableTab>),
    /// A view of its own: the Redis key browser or the MQTT console (`views.rs`).
    View(Box<views::ViewTab>),
}

impl Tab {
    /// The connection this tab belongs to.
    fn ws(&self) -> thaan_core::engine::SessionId {
        match self {
            Tab::Query(q) => q.ws,
            Tab::Table(t) => t.ws,
            Tab::View(v) => v.ws,
        }
    }

    fn title(&self) -> SharedString {
        match self {
            Tab::Query(q) => match &q.database {
                Some(db) => format!("{} · {db}", q.title).into(),
                None => q.title.clone(),
            },
            Tab::Table(t) => match &t.database {
                Some(db) => format!("{} · {db}", t.object.name).into(),
                None => t.object.name.clone().into(),
            },
            Tab::View(v) => v.title.clone(),
        }
    }
}

pub struct ThaanApp {
    engine: Arc<Engine>,
    focus: FocusHandle,
    // connect screen
    drivers: Vec<DriverInfo>,
    driver: DriverInfo,
    conn_name: Entity<InputState>,
    conn_url: Entity<InputState>,
    conn_params: Vec<(String, ParamKind, Entity<InputState>)>,
    /// A picker for each `Choice` parameter; it writes its pick into that parameter's input.
    conn_choices: BTreeMap<String, Entity<SelectState<Vec<SharedString>>>>,
    conn_read_only: bool,
    /// Colour for a new connection: tints its row, dot and the status bar.
    conn_color: Entity<ColorPickerState>,
    /// A connect or test is running on a background thread.
    connecting: bool,
    connections: Vec<ConnectionConfig>,
    // workspace
    session: Option<SessionInfo>,
    /// Other open connections, kept whole while another one is in front.
    parked: Vec<live::Parked>,
    /// The saved connection a connect is running for (its tree row says so).
    connecting_id: Option<String>,
    /// The saved connection whose connect was started by "Open": a query tab opens
    /// for it once connected.
    open_after_connect: Option<String>,
    /// The sidebar row whose right-click menu is open (drawn highlighted).
    menu_row: Option<SharedString>,
    /// Sizes of the sidebar | content split; the status bar's left block follows the
    /// sidebar's width.
    main_split: Entity<gpui_kit::component::ResizableState>,
    /// The open Rename / Duplicate / Truncate / Drop dialog.
    op_dialog: Option<table_ops::OpDialog>,
    /// App preferences (Settings dialog); saved through `Engine::set_settings`.
    settings: thaan_core::store::Settings,
    /// What `thaan mcp` asked to show once this saved connection is connected.
    /// What `thaan mcp` asked to show while a connect was running, in order.
    link_after_connect: Vec<(String, app_link::LinkAction)>,
    /// Settings dialog page and its MCP lookup.
    settings_ui: settings::SettingsUi,
    /// The past query that tab starts with (Recent queries on home), if any.
    open_query_after_connect: Option<SavedQuery>,
    /// The home screen stays in front while connections open behind it; the user goes to
    /// the workspace by opening one (row click), and back with the title bar's Home.
    on_home: bool,
    objects: Vec<SchemaObject>,
    /// Sidebar tree: databases, expanded nodes, other databases' objects.
    tree: ObjectTree,
    tree_filter: Entity<InputState>,
    rail: Rail,
    tabs: Vec<Tab>,
    active: usize,
    next_query: usize,
    palette_open: bool,
    palette_input: Entity<InputState>,
    /// Highlighted row in the palette (index into `palette_layout().0`).
    palette_sel: usize,
    palette_scroll: ScrollHandle,
    status: Option<(SharedString, bool)>,
    /// Home screen and New connection dialog state.
    home: home::HomeState,
}

impl ThaanApp {
    pub fn new(engine: Arc<Engine>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let drivers = engine.drivers();
        // Invariant: main() registers at least one driver.
        let driver = drivers
            .first()
            .cloned()
            .expect("at least one driver is registered");
        let conn_params = Self::param_inputs(&driver, window, cx);
        let conn_choices = Self::choice_pickers(&driver, &conn_params, window, cx);
        let conn_name = cx.new(|cx| InputState::new(window, cx).placeholder("My database"));
        let conn_url = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                "postgres://user@host/db   mysql://…   mongodb://…   redis://…   mqtt://…   sqlite:/path/app.db",
            )
        });
        // A pasted or typed URL fills the fields below it.
        cx.subscribe_in(&conn_url, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.fill_from_url(window, cx);
            }
        })
        .detach();
        let tree_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search commands"));
        cx.subscribe(&tree_filter, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &palette_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                match ev {
                    InputEvent::PressEnter { .. } => {
                        let (items, _) = this.palette_layout(cx);
                        if let Some(c) = items.get(this.palette_sel) {
                            this.run_palette_command(c.id, window, cx);
                        }
                    }
                    InputEvent::Change => {
                        this.palette_sel = 0;
                        this.palette_scroll.scroll_to_item(0);
                    }
                    _ => {}
                }
                cx.notify()
            },
        )
        .detach();
        // Redraw twice a minute so "fetched … ago" in the status bar stays current.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        let connections = engine.connections();
        let mut home = home::HomeState::new(window, cx);
        // In-memory read of the already-loaded store; render never asks the engine.
        home.folders = engine.folders();
        let main_split = cx.new(|_| gpui_kit::component::ResizableState::default());
        // Redraw when the sidebar is resized, so the status bar's left block follows.
        cx.observe(&main_split, |_, _, cx| cx.notify()).detach();
        // A System theme follows the OS switching between light and dark.
        cx.observe_window_appearance(window, |this, window, cx| {
            this.follow_system_appearance(window, cx)
        })
        .detach();
        let settings = engine.settings();
        let mut app = Self {
            settings,
            settings_ui: Default::default(),
            link_after_connect: Vec::new(),
            engine,
            focus: cx.focus_handle(),
            drivers,
            driver,
            conn_name,
            conn_url,
            conn_params,
            conn_choices,
            conn_read_only: false,
            conn_color: cx.new(|cx| ColorPickerState::new(window, cx).default_value(rgb(0x14b8a6))),
            connecting: false,
            connections,
            session: None,
            parked: Vec::new(),
            connecting_id: None,
            open_after_connect: None,
            menu_row: None,
            main_split,
            op_dialog: None,
            open_query_after_connect: None,
            on_home: false,
            objects: Vec::new(),
            tree: ObjectTree::default(),
            tree_filter,
            rail: Rail::Objects,
            tabs: Vec::new(),
            active: 0,
            next_query: 1,
            palette_open: false,
            palette_input,
            palette_sel: 0,
            palette_scroll: ScrollHandle::new(),
            status: None,
            home,
        };
        app.refresh_home_history();
        // `thaan mcp` can open tables and show queries here (app_link.rs).
        app.start_app_link(window, cx);
        app
    }

    /// One input per driver parameter (the form is generated from `DriverInfo::params`).
    fn param_inputs(
        driver: &DriverInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(String, ParamKind, Entity<InputState>)> {
        driver
            .params
            .iter()
            .map(|p| {
                let ph = p.placeholder.clone().unwrap_or_default();
                let st = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(ph);
                    if p.kind == ParamKind::Password {
                        s = s.masked(true);
                    }
                    s
                });
                if let Some(d) = &p.default {
                    st.update(cx, |s, cx| s.set_value(d.clone(), window, cx));
                }
                (p.key.clone(), p.kind, st)
            })
            .collect()
    }

    fn choice_pickers(
        driver: &DriverInfo,
        inputs: &[(String, ParamKind, Entity<InputState>)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BTreeMap<String, Entity<SelectState<Vec<SharedString>>>> {
        let mut out = BTreeMap::new();
        for (spec, (_, _, input)) in driver.params.iter().zip(inputs) {
            if spec.kind != ParamKind::Choice || spec.choices.is_empty() {
                continue;
            }
            let items: Vec<SharedString> = spec
                .choices
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect();
            let current = input.read(cx).value().to_string();
            let ix = spec.choices.iter().position(|c| *c == current).unwrap_or(0);
            let picker = cx.new(|cx| SelectState::new(items, Some(IndexPath::new(ix)), window, cx));
            let input = input.clone();
            cx.subscribe_in(
                &picker,
                window,
                move |_, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(v)) = ev else {
                        return;
                    };
                    input.update(cx, |s, cx| s.set_value(v.to_string(), window, cx));
                    cx.notify();
                },
            )
            .detach();
            out.insert(spec.key.clone(), picker);
        }
        out
    }

    fn select_driver(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.drivers.get(ix).cloned() {
            self.conn_params = Self::param_inputs(&d, window, cx);
            self.conn_choices = Self::choice_pickers(&d, &self.conn_params, window, cx);
            self.driver = d;
            cx.notify();
        }
    }

    /// Parses the URL field and copies what it names into the driver's fields: the driver
    /// itself, then host, port, user, password, database and any `?key=value` options the
    /// driver has a field for. Text that does not parse yet leaves the fields alone.
    fn fill_from_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.conn_url.read(cx).value().trim().to_string();
        let Ok(c) = ConnectionConfig::from_url(&url) else {
            return;
        };
        if c.driver != self.driver.id {
            let Some(ix) = self.drivers.iter().position(|d| d.id == c.driver) else {
                return;
            };
            self.select_driver(ix, window, cx);
        }
        for (key, _, st) in &self.conn_params {
            if let Some(v) = c.params.get(key) {
                if st.read(cx).value().as_ref() != v.as_str() {
                    st.update(cx, |s, cx| s.set_value(v.clone(), window, cx));
                }
            }
        }
        for (key, picker) in &self.conn_choices {
            if let Some(v) = c.params.get(key) {
                let v = SharedString::from(v.clone());
                picker.update(cx, |p, cx| p.set_selected_value(&v, window, cx));
            }
        }
        cx.notify();
    }

    fn is_mongo(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.driver == "mongodb")
    }

    fn set_status(&mut self, msg: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.status = Some((msg.into(), error));
        cx.notify();
    }

    // ---- connections ------------------------------------------------------------------

    fn form_config(&self, cx: &App) -> Result<ConnectionConfig, String> {
        let url = self.conn_url.read(cx).value().trim().to_string();
        let mut c = if !url.is_empty() {
            ConnectionConfig::from_url(&url).map_err(|e| e.to_string())?
        } else {
            let mut params = BTreeMap::new();
            for (k, _, st) in &self.conn_params {
                let v = st.read(cx).value().trim().to_string();
                if !v.is_empty() {
                    params.insert(k.clone(), v);
                }
            }
            ConnectionConfig {
                driver: self.driver.id.clone(),
                params,
                ..Default::default()
            }
        };
        let name = self.conn_name.read(cx).value().trim().to_string();
        if !name.is_empty() {
            c.name = name;
        }
        if c.name.is_empty() {
            // A file path names the connection by its file name, not the whole path.
            c.name = c
                .param("path")
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string())
                })
                .or(c.param("database").map(String::from))
                .unwrap_or_else(|| "connection".into());
        }
        c.read_only = self.conn_read_only;
        c.color = self.conn_color.read(cx).value().map(hex_color);
        Ok(c)
    }

    /// Connects on a background thread: an unreachable host can take until the driver's
    /// timeout, and the window must stay responsive meanwhile.
    fn connect(&mut self, c: ConnectionConfig, window: &mut Window, cx: &mut Context<Self>) {
        // Already open: bring it to the front instead of opening it twice.
        if let Some(sid) = self.live_for(&c.id) {
            return self.activate(sid, cx);
        }
        if self.connecting {
            return;
        }
        self.connecting = true;
        self.connecting_id = Some(c.id.clone());
        self.set_status(format!("Connecting to {}…", c.name), false, cx);
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let info = engine.open(c)?;
                    // Load the schema here too, so a large database does not stall the UI.
                    let objects = engine.list_objects(info.session_id).unwrap_or_default();
                    Ok::<_, thaan_core::Error>((info, objects))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.connecting = false;
                this.connecting_id = None;
                match res {
                    Ok((info, objects)) => {
                        // Connecting only connects: the tree row gets its dot and unfolds,
                        // and the main area stays as it is. Only "Open" (right-click)
                        // also opens a query tab for it.
                        let sid = info.session_id;
                        let info_connection_id = info.connection_id.clone();
                        let open = this.open_after_connect.take().as_deref()
                            == Some(info.connection_id.as_str());
                        this.adopt(info, objects);
                        this.load_databases(sid, cx);
                        let query = this.open_query_after_connect.take();
                        if open {
                            this.activate(sid, cx);
                            let from_history = query.is_some();
                            this.new_query_tab(query, window, cx);
                            if from_history {
                                if let Some(Tab::Query(t)) = this.tabs.last_mut() {
                                    t.saved_id = None;
                                }
                            }
                        }
                        // An agent asked (through `thaan mcp`) to show something here.
                        // Requests for this connection run now; the rest go through
                        // `handle_link` again, which opens their connection in turn.
                        for (cid, action) in std::mem::take(&mut this.link_after_connect) {
                            if cid == info_connection_id {
                                this.activate(sid, cx);
                                this.run_link_action(action, &cid, window, cx);
                            } else {
                                this.handle_link(cid, action, window, cx).ok();
                            }
                        }
                        this.set_status("Connected", false, cx);
                    }
                    Err(e) => {
                        this.link_after_connect.clear();
                        this.open_after_connect = None;
                        this.open_query_after_connect = None;
                        this.set_status(format!("Connection failed: {e}"), true, cx)
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let mut c = match self.form_config(cx) {
            Ok(c) => c,
            Err(e) => return self.set_status(e, true, cx),
        };
        // Editing a saved connection: an empty password box tests with the stored one.
        if let Some(id) = &self.home.editing {
            c.id = id.clone();
        }
        self.connecting = true;
        self.set_status(format!("Testing {}…", c.name), false, cx);
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_spawn(async move { engine.test_connection(&c) })
                .await;
            this.update(cx, |this, cx| {
                this.connecting = false;
                if r.ok {
                    this.set_status(format!("Connection OK ({} ms)", r.elapsed_ms), false, cx);
                } else {
                    this.set_status(r.error.unwrap_or_default(), true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Closes the connection in front; the next open one (or home) takes its place.
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(sid) = self.active_sid() {
            self.close_session(sid, cx);
        }
        self.connections = self.engine.connections();
        cx.notify();
    }

    fn refresh_schema(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let sid = s.session_id;
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.list_objects(sid) })
                .await;
            this.update(cx, |this, cx| {
                // Ignore results for a session that was closed meanwhile.
                if this.session.as_ref().map(|s| s.session_id) != Some(sid) {
                    return;
                }
                match res {
                    Ok(o) => {
                        this.objects = o;
                        this.load_databases(sid, cx);
                    }
                    Err(e) => this.set_status(format!("Schema: {e}"), true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- tabs -------------------------------------------------------------------------

    /// A tree / JSON panel; the app re-renders when its view mode changes.
    fn new_doc_panel(window: &mut Window, cx: &mut Context<Self>) -> Entity<DocPanel> {
        let panel = cx.new(|cx| DocPanel::new(window, cx));
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
        panel
    }

    fn new_grid(window: &mut Window, cx: &mut Context<Self>) -> Entity<TableState<ResultGrid>> {
        let grid = cx.new(|cx| {
            TableState::new(ResultGrid::new(ResultSet::default()), window, cx).sortable(true)
        });
        track_focus(&grid, cx);
        grid
    }

    fn new_query_tab(
        &mut self,
        saved: Option<SavedQuery>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_sid() else { return };
        let language = if self.is_mongo() { "javascript" } else { "sql" };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
        });
        let (title, saved_id) = match saved {
            Some(q) => {
                editor.update(cx, |e, cx| e.set_value(q.sql.clone(), window, cx));
                (q.name.into(), Some(q.id))
            }
            None => {
                let t = format!("Query {}", self.next_query);
                self.next_query += 1;
                (t.into(), None)
            }
        };
        let grid = Self::new_grid(window, cx);
        let docs = Self::new_doc_panel(window, cx);
        self.tabs.push(Tab::Query(Box::new(QueryTab {
            ws,
            sid: ws,
            database: None,
            title,
            saved_id,
            editor: editor.clone(),
            results: Vec::new(),
            active_result: 0,
            grid,
            docs,
            running: false,
        })));
        self.active = self.tabs.len() - 1;
        self.on_home = false;
        editor.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }

    fn open_table(&mut self, object: ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        self.open_table_on(None, object, window, cx)
    }

    /// Opens a table tab. `on` is another database's session of this connection (and
    /// that database's name); `None` reads from the connection's own session. The tab
    /// shows at once; its columns and first page load in the background.
    fn open_table_on(
        &mut self,
        on: Option<(thaan_core::engine::SessionId, String)>,
        object: ObjectRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_sid() else { return };
        let (sid, database) = match on {
            Some((sid, db)) => (sid, Some(db)),
            None => (ws, None),
        };
        if let Some(i) = self.tabs.iter().position(
            |t| matches!(t, Tab::Table(tt) if tt.object == object && tt.sid == sid && tt.ws == ws),
        ) {
            self.active = i;
            self.on_home = false;
            cx.notify();
            return;
        }
        let hint = if self.is_mongo() {
            "Query document, e.g. { age: { $gt: 30 } }"
        } else {
            "WHERE clause, e.g. age > 30"
        };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        cx.subscribe_in(&filter, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                    t.page_ix = 0;
                }
                this.load_page(cx);
            }
        })
        .detach();
        // Columns arrive in the background (`describe_table` can wait on the server);
        // until then only the raw WHERE box works.
        let info: Option<TableInfo> = None;
        let page_size = self.settings.page_size;
        let page_size_input = cx.new(|cx| InputState::new(window, cx));
        page_size_input.update(cx, |s, cx| s.set_value(page_size.to_string(), window, cx));
        // Enter (or leaving the box) applies a typed page size.
        cx.subscribe_in(
            &page_size_input,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let typed = input.read(cx).value().trim().parse::<u64>().ok();
                    this.set_page_size(typed, window, cx);
                }
            },
        )
        .detach();
        let page_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(
            &page_input,
            window,
            |this, input, ev: &InputEvent, _, cx| {
                if !matches!(ev, InputEvent::PressEnter { .. }) {
                    return;
                }
                let typed = input.read(cx).value().trim().parse::<u64>().ok();
                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                    let pages = t.page.as_ref().map_or(1, |p| p.page_count());
                    if let Some(n) = typed {
                        t.page_ix = n.clamp(1, pages) - 1;
                    }
                }
                this.load_page(cx);
            },
        )
        .detach();
        // Table tabs sort on the server, so the order covers every page, not just this one.
        // Cell selection, so a double-click reports which cell to edit; the grid draws
        // its own row numbers, so the built-in row header stays off.
        let grid = cx.new(|cx| {
            TableState::new(ResultGrid::new(ResultSet::default()), window, cx)
                .sortable(true)
                .cell_selectable(true)
                .row_header(false)
        });
        track_focus(&grid, cx);
        // Selecting a row enables the document actions; double-clicking a cell edits it.
        cx.subscribe_in(&grid, window, |this, _, ev: &TableEvent, window, cx| {
            if let TableEvent::DoubleClickedCell(row, col) = ev {
                this.begin_cell_edit(*row, *col, window, cx);
            }
            cx.notify();
        })
        .detach();
        // Staging or dropping a delete from the row menu updates the Apply count.
        cx.observe(&grid, |_, _, cx| cx.notify()).detach();
        let weak = cx.entity().downgrade();
        let sort_object = object.clone();
        let act_on = cx.entity().downgrade();
        grid.update(cx, |g, _| {
            let d = g.delegate_mut();
            d.on_action = Some(std::rc::Rc::new(move |a, window, cx| {
                act_on
                    .update(cx, |app, cx| app.grid_action(a, window, cx))
                    .ok();
            }));
            d.set_server_sort(Box::new(move |column, sort, _, cx| {
                let weak = weak.clone();
                let object = sort_object.clone();
                // Deferred: this runs inside the grid's own update.
                cx.defer(move |cx| {
                    weak.update(cx, |app, cx| app.sort_table(&object, column, sort, cx))
                        .ok();
                });
            }));
        });
        let docs = Self::new_doc_panel(window, cx);
        let projection = cx.new(|cx| InputState::new(window, cx).placeholder("{ field: 1 }"));
        let sort_doc = cx.new(|cx| InputState::new(window, cx).placeholder("{ field: -1 }"));
        for input in [&projection, &sort_doc] {
            cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                        t.page_ix = 0;
                    }
                    this.load_page(cx);
                }
            })
            .detach();
        }
        let starter = command_bar::starter_command(
            self.session.as_ref().map_or("", |s| s.driver.as_str()),
            &object,
        );
        let language = if self.is_mongo() { "javascript" } else { "sql" };
        let command = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
        });
        command.update(cx, |e, cx| e.set_value(starter, window, cx));
        // Keep the "Ln, Col" status line current while typing.
        cx.subscribe(&command, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let describe_object = object.clone();
        self.tabs.push(Tab::Table(Box::new(TableTab {
            object,
            ws,
            sid,
            database,
            info,
            ddl: None,
            struct_tab: Default::default(),
            filter,
            filter_rows: Vec::new(),
            next_filter_id: 1,
            raw_mode: true,
            page_input,
            page_size,
            page_size_input,
            fetched_at: None,
            page: None,
            page_ix: 0,
            sort: Vec::new(),
            view: TableView::Data,
            grid,
            docs,
            projection,
            sort_doc,
            command,
            command_result: None,
            command_running: false,
            // New table and collection tabs open on the data; the query box is one click away.
            command_open: false,
            error: None,
            loading: false,
        })));
        self.active = self.tabs.len() - 1;
        self.on_home = false;
        self.load_page(cx);
        self.describe_in_background(ws, sid, describe_object, window, cx);
    }

    /// Loads a table tab's columns off the UI thread, then turns on the filter builder
    /// and the typed grid headers. Dropped if the tab was closed meanwhile.
    fn describe_in_background(
        &mut self,
        ws: thaan_core::engine::SessionId,
        sid: thaan_core::engine::SessionId,
        object: ObjectRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = self.engine.clone();
        let mongo = self.is_mongo();
        cx.spawn_in(window, async move |this, cx| {
            let target = object.clone();
            let Ok(info) = cx
                .background_spawn(async move { engine.describe_table(sid, &target) })
                .await
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                // The builder needs column names; on MongoDB only the raw box works.
                let builder = !mongo && !info.columns.is_empty();
                let row = builder.then(|| Self::new_filter_row(0, &info, window, cx));
                let Some(Tab::Table(t)) = this.tabs_of(ws).and_then(|tabs| {
                    tabs.iter_mut().find(
                        |t| matches!(t, Tab::Table(tt) if tt.object == object && tt.sid == sid),
                    )
                }) else {
                    return;
                };
                t.grid.update(cx, |g, cx| {
                    g.delegate_mut().set_table_info(&info);
                    g.refresh(cx);
                });
                // Switch to the builder only if nothing was typed in the raw box yet.
                if let Some(row) = row {
                    if t.filter_rows.is_empty() && t.filter.read(cx).value().trim().is_empty() {
                        t.filter_rows.push(row);
                        t.raw_mode = false;
                    }
                }
                t.info = Some(info);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Header click on a table tab: refetch page 1 sorted by `column` on the server.
    fn sort_table(
        &mut self,
        object: &ObjectRef,
        column: String,
        sort: ColumnSort,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Table(tt) if &tt.object == object))
        else {
            return;
        };
        if let Some(Tab::Table(t)) = self.tabs.get_mut(ix) {
            t.sort = match sort {
                ColumnSort::Ascending => vec![Sort {
                    column,
                    descending: false,
                }],
                ColumnSort::Descending => vec![Sort {
                    column,
                    descending: true,
                }],
                _ => Vec::new(),
            };
            t.page_ix = 0;
        }
        // `load_page` fetches the active tab; a header click always comes from it.
        if ix == self.active {
            self.load_page(cx);
        }
    }

    fn new_filter_row(
        id: usize,
        info: &TableInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FilterRow {
        let columns: Vec<SharedString> = info
            .columns
            .iter()
            .map(|c| SharedString::from(c.name.clone()))
            .collect();
        let ops: Vec<SharedString> = FilterOp::PICKER
            .iter()
            .map(|o| SharedString::from(o.title()))
            .collect();
        let column = cx.new(|cx| {
            SelectState::new(columns, Some(IndexPath::new(0)), window, cx).searchable(true)
        });
        let op = cx.new(|cx| SelectState::new(ops, Some(IndexPath::new(0)), window, cx));
        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        // Changing the operator can hide or show the value box.
        cx.subscribe_in(
            &op,
            window,
            |_, _, _: &SelectEvent<Vec<SharedString>>, _, cx| cx.notify(),
        )
        .detach();
        cx.subscribe_in(&value, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.apply_filter(cx);
            }
        })
        .detach();
        FilterRow {
            id,
            column,
            op,
            value,
        }
    }

    fn add_filter_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(info) = t.info.clone() else { return };
        let id = t.next_filter_id;
        t.next_filter_id += 1;
        let row = Self::new_filter_row(id, &info, window, cx);
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            row.value.update(cx, |v, cx| v.focus(window, cx));
            t.filter_rows.push(row);
        }
        cx.notify();
    }

    fn remove_filter_row(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            t.filter_rows.retain(|r| r.id != id);
        }
        self.apply_filter(cx);
    }

    /// A request from a table grid's cell menu.
    fn grid_action(
        &mut self,
        a: crate::grid::GridAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match a {
            crate::grid::GridAction::Edit { row, col } => {
                self.begin_cell_edit(row, col, window, cx)
            }
            crate::grid::GridAction::Filter { column, op, value } => {
                let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) else {
                    return;
                };
                let Some(r) = t.filter_rows.first() else {
                    return;
                };
                let (col_pick, op_pick, input) = (r.column.clone(), r.op.clone(), r.value.clone());
                t.raw_mode = false;
                col_pick.update(cx, |s, cx| {
                    s.set_selected_value(&SharedString::from(column), window, cx)
                });
                op_pick.update(cx, |s, cx| {
                    s.set_selected_value(&SharedString::from(op.title()), window, cx)
                });
                let text = if value.is_null() {
                    String::new()
                } else {
                    value.to_string()
                };
                input.update(cx, |s, cx| s.set_value(text, window, cx));
                self.apply_filter(cx);
            }
            other => self.row_action(other, window, cx),
        }
    }

    /// Re-fetches from page 1 with the current filter.
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            t.page_ix = 0;
        }
        self.load_page(cx);
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.tabs.remove(ix);
            // Keep the same tab on screen when one before it closes.
            if ix < self.active {
                self.active -= 1;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
            // The connection in front follows the tab now on screen.
            if !self.on_home {
                if let Some(ws) = self.tabs.get(self.active).map(Tab::ws) {
                    self.activate(ws, cx);
                }
            }
            cx.notify();
        }
    }

    // ---- running queries -------------------------------------------------------------

    fn run(&mut self, current_only: bool, cx: &mut Context<Self>) {
        // ⌘↵ in a table tab runs its command box.
        if matches!(self.tabs.get(self.active), Some(Tab::Table(_))) {
            return self.run_table_command(cx);
        }
        let Some(session) = &self.session else { return };
        let sid = session.session_id;
        let ix = self.active;
        let Some(Tab::Query(q)) = self.tabs.get_mut(ix) else {
            return;
        };
        if q.running {
            return;
        }
        // `sid` finds the tab again; `run_on` is the session the SQL runs on.
        let run_on = q.sid;
        let ed = q.editor.read(cx);
        let sql = ed.value().to_string();
        let sel = ed.selected_range();
        let scope = if !sel.is_empty() {
            RunScope::Range {
                start: sel.start,
                end: sel.end,
            }
        } else if current_only {
            RunScope::Current {
                cursor: ed.cursor(),
            }
        } else {
            RunScope::All
        };
        q.running = true;
        cx.notify();
        let engine = self.engine.clone();
        let req = QueryRequest {
            session_id: run_on,
            sql,
            scope,
            params: BTreeMap::new(),
        };
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update(cx, |this, cx| {
                // The tab may sit behind another connection by now; it still gets its result.
                let in_front = this.active_sid() == Some(sid);
                let Some(Tab::Query(q)) = this.tab_at(sid, ix) else {
                    return;
                };
                q.running = false;
                let res = res;
                this.refresh_home_history();
                let Some(Tab::Query(q)) = this.tab_at(sid, ix) else {
                    return;
                };
                match res {
                    Ok(results) => {
                        let last_ok = results.iter().rposition(|r| {
                            r.result.as_ref().is_some_and(|r| !r.columns.is_empty())
                        });
                        q.active_result = last_ok.unwrap_or(results.len().saturating_sub(1));
                        let msg = summarize(&results);
                        q.results = results;
                        let rs = q
                            .results
                            .get(q.active_result)
                            .and_then(|r| r.result.clone())
                            .unwrap_or_default();
                        let documents = rs.documents.clone().unwrap_or_default();
                        q.docs.update(cx, |p, cx| p.set_docs(documents, cx));
                        q.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(rs);
                            g.refresh(cx);
                        });
                        let failed = q.results.iter().any(|r| r.error.is_some());
                        // DDL may have changed the schema.
                        let ddl = q.results.iter().any(|r| is_ddl(&r.sql));
                        if in_front {
                            if ddl {
                                this.refresh_schema(cx);
                            }
                            this.set_status(msg, failed, cx);
                        }
                    }
                    Err(e) if in_front => this.set_status(e.to_string(), true, cx),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_result(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(Tab::Query(q)) = self.tabs.get_mut(self.active) {
            q.active_result = i;
            let rs = q
                .results
                .get(i)
                .and_then(|r| r.result.clone())
                .unwrap_or_default();
            let documents = rs.documents.clone().unwrap_or_default();
            q.docs.update(cx, |p, cx| p.set_docs(documents, cx));
            q.grid.update(cx, |g, cx| {
                g.delegate_mut().set(rs);
                g.refresh(cx);
            });
            cx.notify();
        }
    }

    fn load_page(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        // `ws` finds the tab again when the answer arrives; `sid` is the session it reads.
        let ws = session.session_id;
        let writable = !session.read_only;
        let ix = self.active;
        let Some(Tab::Table(t)) = self.tabs.get_mut(ix) else {
            return;
        };
        let sid = t.sid;
        // Paging or filtering leaves a command result for the paged view.
        t.command_result = None;
        let (filters, raw_where) = t.where_parts(cx);
        let (projection, raw_sort) = t.document_shape(cx);
        let req = PageRequest {
            table: t.object.clone(),
            filters,
            raw_where,
            projection,
            raw_sort,
            sort: t.sort.clone(),
            page: t.page_ix,
            page_size: t.page_size,
        };
        t.loading = true;
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.fetch_page(sid, &req) })
                .await;
            this.update(cx, |this, cx| {
                let Some(Tab::Table(t)) = this.tab_at(ws, ix) else {
                    return;
                };
                t.loading = false;
                match res {
                    Ok(page) => {
                        t.error = None;
                        t.fetched_at = Some(std::time::Instant::now());
                        let rs = page.result.clone();
                        let offset = page.page * page.page_size;
                        t.page = Some(page);
                        let documents = rs.documents.clone().unwrap_or_default();
                        t.docs.update(cx, |p, cx| p.set_docs(documents, cx));
                        t.grid.update(cx, |g, cx| {
                            g.delegate_mut().set_row_offset(offset);
                            g.delegate_mut().set(rs);
                            g.delegate_mut().editable = writable;
                            g.refresh(cx);
                        });
                    }
                    Err(e) => t.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current_result(&self) -> Option<ResultSet> {
        match self.tabs.get(self.active)? {
            Tab::Query(q) => q.results.get(q.active_result)?.result.clone(),
            Tab::Table(t) => t.page.as_ref().map(|p| p.result.clone()),
            Tab::View(_) => None,
        }
    }

    /// ⌘C in the grid in front: the selected row as TSV, or the selected cell's value.
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let grid = match self.tabs.get(self.active) {
            Some(Tab::Table(t)) => t.grid.clone(),
            Some(Tab::Query(q)) => q.grid.clone(),
            Some(Tab::View(_)) | None => return,
        };
        let Some(text) = grid.read(cx).delegate().selection_text() else {
            return;
        };
        let what = if text.contains('\t') {
            "Copied row"
        } else {
            "Copied"
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.set_status(what, false, cx);
    }

    fn copy_result(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let (Some(s), Some(rs)) = (&self.session, self.current_result()) else {
            return;
        };
        let mut opts = ExportOptions::new(format);
        if let Some(Tab::Table(t)) = self.tabs.get(self.active) {
            opts.table_name = Some(t.object.name.clone());
        }
        match self.engine.export_result(s.session_id, &rs, &opts) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status(
                    format!(
                        "Copied {} as {}",
                        plural(rs.rows.len() as u64, "row"),
                        format.label()
                    ),
                    false,
                    cx,
                );
            }
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn export_table_to_file(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        let (filters, raw_where) = t.where_parts(cx);
        let (projection, raw_sort) = t.document_shape(cx);
        let req = PageRequest {
            table: t.object.clone(),
            filters,
            raw_where,
            projection,
            raw_sort,
            sort: t.sort.clone(),
            page: 0,
            page_size: 100,
        };
        let dir = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let path = dir.join(format!("{}.{}", t.object.name, format.extension()));
        let r = self
            .engine
            .export_table(t.sid, &req, &ExportOptions::new(format))
            .and_then(|text| std::fs::write(&path, text).map_err(Into::into));
        match r {
            Ok(()) => self.set_status(format!("Exported to {}", path.display()), false, cx),
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn save_current_query(&mut self, cx: &mut Context<Self>) {
        let Some(Tab::Query(q)) = self.tabs.get_mut(self.active) else {
            return;
        };
        let sql = q.editor.read(cx).value().to_string();
        let saved = SavedQuery {
            id: q.saved_id.clone().unwrap_or_default(),
            name: q.title.to_string(),
            folder: None,
            sql,
            connection_id: self.session.as_ref().map(|s| s.connection_id.clone()),
            updated_ms: 0,
        };
        match self.engine.save_query(saved) {
            Ok(s) => {
                q.saved_id = Some(s.id);
                self.set_status(format!("Saved “{}”", s.name), false, cx);
            }
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        self.palette_sel = 0;
        if self.palette_open {
            self.palette_input.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Commands in display order (grouped by category) and, for each, its child index in
    /// the scrolling list, which also holds one label row per group. With an empty search
    /// only commands this window can run are listed; a search also finds CLI-only ones.
    fn palette_layout(&self, cx: &App) -> (Vec<CommandSpec>, Vec<usize>) {
        let q = self.palette_input.read(cx).value().trim().to_lowercase();
        let matches: Vec<CommandSpec> = builtin_commands()
            .into_iter()
            .filter(|c| {
                if q.is_empty() {
                    // Without a session, hide what needs one (running queries, tabs…).
                    // Clearing history is left to search, so Enter on an unchanged
                    // list never erases anything.
                    gui_runs(c.id)
                        && (self.session.is_some()
                            || matches!(c.id, "app.toggleTheme" | "app.settings" | "data.transfer"))
                } else {
                    c.title.to_lowercase().contains(&q)
                        || c.id.to_lowercase().contains(&q)
                        || c.category.to_lowercase().contains(&q)
                }
            })
            .collect();
        let mut categories: Vec<&str> = Vec::new();
        for c in &matches {
            if !categories.contains(&c.category) {
                categories.push(c.category);
            }
        }
        let mut items = Vec::new();
        let mut child_ix = Vec::new();
        let mut next = 0;
        for cat in categories {
            next += 1; // the group label
            for c in matches.iter().filter(|c| c.category == cat) {
                items.push(c.clone());
                child_ix.push(next);
                next += 1;
            }
        }
        (items, child_ix)
    }

    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (items, child_ix) = self.palette_layout(cx);
        if items.is_empty() {
            return;
        }
        let n = items.len() as isize;
        self.palette_sel = (self.palette_sel as isize + delta).rem_euclid(n) as usize;
        // Keep the group label in view when the first row of a group is selected.
        let ix = child_ix[self.palette_sel];
        self.palette_scroll
            .scroll_to_item(if ix > 0 && child_ix.first() == Some(&ix) {
                0
            } else {
                ix
            });
        cx.notify();
    }

    fn run_palette_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        window.focus(&self.focus, cx);
        match id {
            "query.runAll" => self.run(false, cx),
            "query.runCurrent" | "query.runSelection" => self.run(true, cx),
            "tab.new" => self.new_query_tab(None, window, cx),
            "tab.close" => self.close_tab(self.active, cx),
            // Starts from the connection in front, if any.
            "data.transfer" => {
                let source = self
                    .session
                    .as_ref()
                    .map(|s| (s.connection_id.clone(), None));
                self.open_transfer_dialog(source, window, cx)
            }
            "schema.refresh" | "schema.listObjects" => self.refresh_schema(cx),
            "connection.close" => self.disconnect(cx),
            "app.toggleTheme" => self.toggle_theme(window, cx),
            "app.settings" => self.open_settings(window, cx),
            "savedQuery.save" => self.save_current_query(cx),
            "history.list" => self.rail = Rail::History,
            "savedQuery.list" => self.rail = Rail::Saved,
            "result.export" => self.copy_result(ExportFormat::Csv, cx),
            "history.clear" => {
                self.engine.clear_history().ok();
                self.refresh_home_history();
            }
            other => self.set_status(
                format!("`{other}` is available from the CLI: thaan run {other}"),
                false,
                cx,
            ),
        }
        cx.notify();
    }
}

/// Commands this window carries out itself; keep in step with `run_palette_command`.
fn gui_runs(id: &str) -> bool {
    matches!(
        id,
        "query.runAll"
            | "query.runCurrent"
            | "query.runSelection"
            | "tab.new"
            | "tab.close"
            | "schema.refresh"
            | "schema.listObjects"
            | "connection.close"
            | "app.toggleTheme"
            | "app.settings"
            | "savedQuery.save"
            | "history.list"
            | "savedQuery.list"
            | "result.export"
            | "history.clear"
            | "data.transfer"
    )
}

fn is_ddl(sql: &str) -> bool {
    let l = sql.trim_start().to_ascii_lowercase();
    ["create", "drop", "alter", "rename"]
        .iter()
        .any(|k| l.starts_with(k))
}

fn summarize(results: &[StatementResult]) -> String {
    if let Some(r) = results.iter().find(|r| r.error.is_some()) {
        return format!("Error: {}", r.error.as_deref().unwrap_or_default());
    }
    let ms: u64 = results
        .iter()
        .filter_map(|r| r.result.as_ref())
        .map(|r| r.elapsed_ms)
        .sum();
    let stmts = if results.len() > 1 {
        format!("{} · ", plural(results.len() as u64, "statement"))
    } else {
        String::new()
    };
    match results.last().and_then(|r| r.result.as_ref()) {
        Some(rs) if !rs.columns.is_empty() => {
            format!("{stmts}{} · {ms} ms", plural(rs.rows.len() as u64, "row"))
        }
        Some(rs) => format!(
            "{stmts}{} affected · {ms} ms",
            plural(rs.rows_affected.unwrap_or(0), "row")
        ),
        None => "No statements".into(),
    }
}

// ---- rendering ------------------------------------------------------------------------

impl ThaanApp {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let filter = self.tree_filter.read(cx).value().to_lowercase();
        let rail_btn = |id: &'static str,
                        ic: &'static str,
                        tip: &'static str,
                        r: Rail,
                        cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon(ic))
                .tooltip(tip)
                .selected(self.rail == r)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rail = r;
                    cx.notify();
                }))
        };
        let rail = v_flex()
            .w(px(40.))
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(rail_btn(
                "rail-obj",
                "database",
                "Objects",
                Rail::Objects,
                cx,
            ))
            .child(rail_btn(
                "rail-saved",
                "bookmark",
                "Saved queries",
                Rail::Saved,
                cx,
            ))
            .child(rail_btn("rail-hist", "clock", "History", Rail::History, cx))
            .child(div().flex_1())
            .child(
                Button::new("theme")
                    .ghost()
                    .small()
                    .icon(icon("sun-moon"))
                    .tooltip("Toggle theme")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
            )
            .child(
                Button::new("disc")
                    .ghost()
                    .small()
                    .icon(icon("unplug"))
                    .tooltip("Disconnect all")
                    .disabled(self.session.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_all(cx))),
            );

        let content: AnyElement = match self.rail {
            Rail::Objects => v_flex()
                .size_full()
                .child(
                    h_flex()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .gap_1()
                        .child(div().flex_1().child(Input::new(&self.tree_filter).small()))
                        .child(
                            Button::new("refresh")
                                .ghost()
                                .xsmall()
                                .icon(icon("refresh-cw"))
                                .tooltip("Refresh (⌘R)")
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_schema(cx))),
                        )
                        .child(
                            Button::new("tree-new-group")
                                .ghost()
                                .xsmall()
                                .icon(icon("folder-plus"))
                                .tooltip("New group")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_group_dialog(window, cx)
                                })),
                        ),
                )
                .child(self.render_object_tree(&filter, cx))
                .into_any_element(),
            Rail::Saved => {
                let items = self.engine.saved_queries();
                list_panel(
                    "Saved queries",
                    items.len(),
                    "Save a query with the bookmark button in its tab",
                    &theme,
                )
                .children(items.into_iter().enumerate().map(|(i, q)| {
                    let q2 = q.clone();
                    let id = q.id.clone();
                    h_flex()
                        .id(("saved", i))
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.sidebar_accent))
                        .child(
                            icon("file-code")
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                .gap_0p5()
                                .child(div().text_sm().truncate().child(q.name.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(first_line(&q.sql)),
                                ),
                        )
                        .child(
                            Button::new(("saved-del", i))
                                .ghost()
                                .xsmall()
                                .icon(icon("trash"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.engine.delete_saved_query(&id).ok();
                                    cx.stop_propagation();
                                    cx.notify();
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_query_tab(Some(q2.clone()), window, cx)
                        }))
                }))
                .into_any_element()
            }
            Rail::History => {
                let items: Vec<HistoryEntry> = self.engine.history(200);
                list_panel(
                    "History",
                    items.len(),
                    "Queries you run appear here",
                    &theme,
                )
                .children(items.into_iter().enumerate().map(|(i, h)| {
                    let sql = h.sql.clone();
                    let failed = h.error.is_some();
                    h_flex()
                        .id(("hist", i))
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.sidebar_accent))
                        .child(
                            div()
                                .size_4()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(6.)).rounded_full().bg(if failed {
                                    theme.danger
                                } else {
                                    theme.muted_foreground.opacity(0.5)
                                })),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
                                        .child(first_line(&h.sql)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(if failed {
                                            "Failed".to_string()
                                        } else {
                                            format!(
                                                "{} · {} ms",
                                                plural(h.rows.unwrap_or(0), "row"),
                                                h.elapsed_ms
                                            )
                                        }),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let q = SavedQuery {
                                id: String::new(),
                                name: "From history".into(),
                                folder: None,
                                sql: sql.clone(),
                                connection_id: None,
                                updated_ms: 0,
                            };
                            this.new_query_tab(Some(q), window, cx);
                            if let Some(Tab::Query(t)) = this.tabs.last_mut() {
                                t.saved_id = None;
                            }
                        }))
                }))
                .into_any_element()
            }
        };

        h_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(rail)
            .child(div().flex_1().h_full().overflow_hidden().child(content))
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("tabs")
            .h(px(36.))
            .w_full()
            .overflow_x_scroll()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child({
                // Home: the overview, one tab away from the open tables and queries.
                let home = self.on_home || self.tabs.is_empty();
                h_flex()
                    .id("tab-home")
                    .h_full()
                    .gap_2()
                    .px_3()
                    .text_sm()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(home, |d| {
                        d.bg(theme.background)
                            .border_b_2()
                            .border_color(theme.primary)
                    })
                    .when(!home, |d| {
                        d.text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.background.opacity(0.5)))
                    })
                    .child(icon("house").size_3p5())
                    .child("Home")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.on_home = true;
                        cx.notify();
                    }))
            })
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let active = i == self.active && !self.on_home;
                // Tabs of every connection share the strip; the dot says whose it is.
                let owner = self
                    .session_info(t.ws())
                    .and_then(|s| s.color.as_deref())
                    .and_then(parse_hex)
                    .unwrap_or(theme.primary);
                let ic = match t {
                    Tab::Query(_) => "file-code",
                    Tab::Table(_) => "table",
                    Tab::View(v) => v.kind.icon(),
                };
                h_flex()
                    .id(("tab", i))
                    .h_full()
                    .gap_2()
                    .pl_3()
                    .pr_1()
                    .text_sm()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme.background)
                            .border_b_2()
                            .border_color(theme.primary)
                    })
                    .when(!active, |d| {
                        d.text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.background.opacity(0.5)))
                    })
                    .child(conn_dot(owner))
                    .child(icon(ic).size_3p5())
                    .child(t.title())
                    .child(
                        Button::new(("close", i))
                            .ghost()
                            .xsmall()
                            .icon(icon("x"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show_tab(i, cx)))
            }))
            .child(
                Button::new("new-tab")
                    .ghost()
                    .small()
                    .icon(icon("plus"))
                    .tooltip("New query (⌘T)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_query_tab(None, window, cx)),
                    ),
            )
    }

    fn render_results_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_1()
            .child(div().pr_1().text_xs().text_color(muted).child("Copy as"))
            .child(
                Button::new("copy-csv")
                    .ghost()
                    .xsmall()
                    .label("CSV")
                    .tooltip("Copy as CSV")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Csv, cx)),
                    ),
            )
            .child(
                Button::new("copy-json")
                    .ghost()
                    .xsmall()
                    .label("JSON")
                    .tooltip("Copy as JSON")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Json, cx)),
                    ),
            )
            .child(
                Button::new("copy-md")
                    .ghost()
                    .xsmall()
                    .label("MD")
                    .tooltip("Copy as Markdown")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Markdown, cx)),
                    ),
            )
            .child(
                Button::new("copy-sql")
                    .ghost()
                    .xsmall()
                    .label("SQL")
                    .tooltip("Copy as INSERT statements")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::SqlInsert, cx)),
                    ),
            )
    }

    fn render_query_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(Tab::Query(q)) = self.tabs.get(self.active) else {
            return div().into_any_element();
        };
        let running = q.running;
        let toolbar = h_flex()
            .h(px(36.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("run")
                    .primary()
                    .small()
                    .icon(icon("play"))
                    .label(if running { "Running…" } else { "Run" })
                    .tooltip("Run current statement or selection (⌘↵)")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.run(true, cx))),
            )
            .child(
                Button::new("run-all")
                    .small()
                    .label("Run all")
                    .tooltip("Run every statement (⌘⇧↵)")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.run(false, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("save-q")
                    .ghost()
                    .small()
                    .icon(icon("bookmark"))
                    .tooltip("Save query")
                    .on_click(cx.listener(|this, _, _, cx| this.save_current_query(cx))),
            );

        let result_tabs = h_flex()
            .gap_1()
            .children(q.results.iter().enumerate().map(|(i, r)| {
                let label = match (&r.result, &r.error) {
                    (_, Some(_)) => format!("{} · Error", i + 1),
                    (Some(rs), _) if !rs.columns.is_empty() => {
                        format!("{} · {}", i + 1, plural(rs.rows.len() as u64, "row"))
                    }
                    (Some(rs), _) => format!(
                        "{} · {} affected",
                        i + 1,
                        plural(rs.rows_affected.unwrap_or(0), "row")
                    ),
                    _ => format!("{}", i + 1),
                };
                Button::new(("res", i))
                    .xsmall()
                    .ghost()
                    .label(label)
                    .selected(i == q.active_result)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_result(i, cx)))
            }));

        let active = q.results.get(q.active_result);
        let body: AnyElement = match active {
            None => empty_state("Run a query to see results here", &theme),
            Some(r) if r.error.is_some() => div()
                .p_4()
                .text_sm()
                .text_color(theme.danger)
                .font_family(theme.mono_font_family.clone())
                .child(r.error.clone().unwrap_or_default())
                .into_any_element(),
            Some(r) if r.result.as_ref().is_some_and(|rs| rs.columns.is_empty()) => empty_state(
                &format!(
                    "{} affected",
                    plural(
                        r.result.as_ref().and_then(|x| x.rows_affected).unwrap_or(0),
                        "row"
                    )
                ),
                &theme,
            ),
            Some(r)
                if r.result.as_ref().is_some_and(|rs| rs.documents.is_some())
                    && q.docs.read(cx).view() != DocView::Table =>
            {
                q.docs.clone().into_any_element()
            }
            Some(_) => DataTable::new(&q.grid)
                .stripe(true)
                .bordered(false)
                .header_row_height(px(GRID_HEADER_H))
                .into_any_element(),
        };
        let doc_switch = active
            .and_then(|r| r.result.as_ref())
            .is_some_and(|rs| rs.documents.is_some())
            .then(|| view_switch(&q.docs, cx));

        v_resizable("query-split")
            .child(
                resizable_panel().size(px(260.)).child(
                    v_flex().size_full().child(toolbar).child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(Editor::new(&q.editor).bordered(false).h_full()),
                    ),
                ),
            )
            .child(
                resizable_panel().child(
                    v_flex()
                        .size_full()
                        .child(
                            h_flex()
                                .h(px(36.))
                                .px_2()
                                .gap_2()
                                .border_b_1()
                                .border_color(theme.border)
                                .bg(theme.sidebar)
                                .child(result_tabs)
                                .child(div().flex_1())
                                .children(doc_switch)
                                .child(self.render_results_toolbar(cx)),
                        )
                        .child(div().flex_1().overflow_hidden().child(body)),
                ),
            )
            .into_any_element()
    }

    fn render_table_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mongo = self.is_mongo();
        let theme = cx.theme().clone();
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return div().into_any_element();
        };
        let view = t.view;
        let raw_mode = t.raw_mode;
        // Rows exist only when the builder can work (known columns, not MongoDB).
        let can_build = !t.filter_rows.is_empty();
        let seg = |id: &'static str, label: &'static str, v: TableView, cx: &mut Context<Self>| {
            Button::new(id)
                .xsmall()
                .ghost()
                .label(label)
                .selected(view == v)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                        t.view = v;
                    }
                    // `ddl` can wait on the server, so it loads in the background.
                    if v == TableView::Structure {
                        this.load_structure_ddl(false, cx);
                    }
                    cx.notify();
                }))
        };
        let cmd_open = t.command_open;
        let header = h_flex()
            .h(px(36.))
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(seg("v-data", "Data", TableView::Data, cx))
            .child(seg("v-struct", "Structure", TableView::Structure, cx))
            .child(div().flex_1())
            .when(view == TableView::Data, |d| {
                d.child(
                    Button::new("cmd-toggle")
                        .ghost()
                        .xsmall()
                        .icon(icon(if cmd_open {
                            "panel-top-close"
                        } else {
                            "panel-top-open"
                        }))
                        .label("Query")
                        .selected(cmd_open)
                        .tooltip(if cmd_open {
                            "Hide the query box"
                        } else {
                            "Show the query box for this table"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                t.command_open = !t.command_open;
                            }
                            cx.notify();
                        })),
                )
            })
            .when(view == TableView::Data && can_build, |d| {
                d.child(
                    Button::new("filter-mode")
                        .ghost()
                        .xsmall()
                        .icon(icon(if raw_mode {
                            "list-filter"
                        } else {
                            "square-code"
                        }))
                        .label(if raw_mode {
                            "Filter builder"
                        } else {
                            "WHERE clause"
                        })
                        .tooltip(if raw_mode {
                            "Filter by column, operator and value"
                        } else {
                            "Type a raw WHERE clause"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                t.raw_mode = !t.raw_mode;
                            }
                            this.apply_filter(cx);
                        })),
                )
            });

        const APPLY_W: f32 = 64.;
        let apply = |cx: &mut Context<Self>| {
            Button::new("apply-filter")
                .small()
                .w(px(APPLY_W))
                .label("Apply")
                .tooltip("Apply the filter (↵)")
                .on_click(cx.listener(|this, _, _, cx| this.apply_filter(cx)))
        };
        let filter_bar: Option<AnyElement> = (view == TableView::Data).then(|| {
            if mongo {
                // MongoDB: filter, projection and sort documents (docs/spec/mongodb-ui.md M6).
                let labelled = |label: &'static str, input: &Entity<InputState>, grow: f32| {
                    h_flex()
                        .flex_grow(grow)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .gap_1p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                };
                h_flex()
                    .h(px(40.))
                    .px_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(labelled("Filter", &t.filter, 0.5))
                    .child(labelled("Projection", &t.projection, 0.25))
                    .child(labelled("Sort", &t.sort_doc, 0.25))
                    .child(apply(cx))
                    .child(view_switch(&t.docs, cx))
                    .into_any_element()
            } else if raw_mode {
                h_flex()
                    .h(px(40.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(Input::new(&t.filter).small()))
                    .child(apply(cx))
                    .into_any_element()
            } else {
                let last = t.filter_rows.len().saturating_sub(1);
                v_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .children(t.filter_rows.iter().enumerate().map(|(i, r)| {
                        let id = r.id;
                        let needs_value =
                            r.op.read(cx)
                                .selected_value()
                                .and_then(|l| FilterOp::from_title(l))
                                .is_none_or(FilterOp::needs_value);
                        h_flex()
                            .h(px(32.))
                            .gap_2()
                            .child(
                                div()
                                    .w(px(40.))
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if i == 0 { "Where" } else { "and" }),
                            )
                            // The select fills its parent, so fixed-width boxes set the width.
                            .child(
                                div()
                                    .w(px(200.))
                                    .flex_none()
                                    .child(Select::new(&r.column).small()),
                            )
                            .child(
                                div()
                                    .w(px(176.))
                                    .flex_none()
                                    .child(Select::new(&r.op).small()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .when(needs_value, |d| d.child(Input::new(&r.value).small())),
                            )
                            .child(div().w(px(24.)).when(t.filter_rows.len() > 1, |d| {
                                d.child(
                                    Button::new(("filter-del", id))
                                        .ghost()
                                        .xsmall()
                                        .icon(icon("minus"))
                                        .tooltip("Remove this condition")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_filter_row(id, cx)
                                        })),
                                )
                            }))
                            .child(div().w(px(24.)).when(i == last, |d| {
                                d.child(
                                    Button::new("filter-add")
                                        .ghost()
                                        .xsmall()
                                        .icon(icon("plus"))
                                        .tooltip("Add a condition")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_filter_row(window, cx)
                                        })),
                                )
                            }))
                            .child(div().w(px(APPLY_W)).when(i == 0, |d| d.child(apply(cx))))
                    }))
                    .into_any_element()
            }
        });

        let body: AnyElement = match view {
            TableView::Data => {
                let grid: AnyElement = match &t.error {
                    Some(e) => div()
                        .p_4()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(e.clone())
                        .into_any_element(),
                    None if mongo && t.docs.read(cx).view() != DocView::Table => {
                        t.docs.clone().into_any_element()
                    }
                    None => DataTable::new(&t.grid)
                        .stripe(true)
                        .bordered(false)
                        .header_row_height(px(GRID_HEADER_H))
                        .into_any_element(),
                };
                v_flex()
                    .size_full()
                    .child(div().flex_1().overflow_hidden().child(grid))
                    .into_any_element()
            }
            // SQL tables: Columns · Indexes · Relations · Triggers · DDL (structure.rs).
            TableView::Structure if !self.is_mongo() => self.render_structure(t, cx),
            TableView::Structure => {
                let info = t.info.clone();
                let ddl = t.ddl.clone().unwrap_or_default();
                let mono = theme.mono_font_family.clone();
                // Each section is its label plus its content, 8 apart; sections sit 32 apart.
                let section = |title: &str, n: Option<usize>, body: AnyElement| {
                    v_flex()
                        .gap_2()
                        .child(group_label(title, n, &theme))
                        .child(body)
                };
                v_flex()
                    .id("structure")
                    .size_full()
                    .overflow_y_scroll()
                    .p_6()
                    .gap_8()
                    .when_some(info, |d, info| {
                        d.child(section(
                            "Columns",
                            Some(info.columns.len()),
                            v_flex()
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(8.))
                                .child(struct_row(
                                    ["#", "Name", "Type", "Nullable", "Default", "Key"]
                                        .map(String::from),
                                    true,
                                    &theme,
                                ))
                                .children(info.columns.iter().enumerate().map(|(i, c)| {
                                    struct_row(
                                        [
                                            (i + 1).to_string(),
                                            c.name.clone(),
                                            c.data_type.clone(),
                                            if c.nullable {
                                                "Yes".into()
                                            } else {
                                                "No".into()
                                            },
                                            c.default.clone().unwrap_or_default(),
                                            c.primary_key
                                                .map(|p| format!("PK {p}"))
                                                .unwrap_or_default(),
                                        ],
                                        false,
                                        &theme,
                                    )
                                }))
                                .into_any_element(),
                        ))
                        .when(!info.indexes.is_empty(), |d| {
                            d.child(section(
                                "Indexes",
                                Some(info.indexes.len()),
                                v_flex()
                                    .gap_1()
                                    .children(info.indexes.iter().map(|ix| {
                                        div().text_sm().font_family(mono.clone()).child(format!(
                                            "{}{} ({})",
                                            ix.name,
                                            if ix.unique { " UNIQUE" } else { "" },
                                            ix.columns.join(", ")
                                        ))
                                    }))
                                    .into_any_element(),
                            ))
                        })
                        .when(!info.foreign_keys.is_empty(), |d| {
                            d.child(section(
                                "Foreign keys",
                                Some(info.foreign_keys.len()),
                                v_flex()
                                    .gap_1()
                                    .children(info.foreign_keys.iter().map(|fk| {
                                        div().text_sm().font_family(mono.clone()).child(format!(
                                            "({}) → {} ({})",
                                            fk.columns.join(", "),
                                            fk.ref_table,
                                            fk.ref_columns.join(", ")
                                        ))
                                    }))
                                    .into_any_element(),
                            ))
                        })
                    })
                    .child(section(
                        "DDL",
                        None,
                        div()
                            .p_4()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.sidebar)
                            .text_sm()
                            .font_family(mono.clone())
                            .child(ddl)
                            .into_any_element(),
                    ))
                    .into_any_element()
            }
        };
        let rest = v_flex()
            .size_full()
            .children(filter_bar)
            .children(self.render_doc_bar(cx))
            .children(self.render_row_bar(cx))
            .child(
                h_flex().flex_1().min_h_0().overflow_hidden().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(body),
                ),
            );
        // The command box and the grid share the height; drag the divider to resize.
        let main: AnyElement = match self.render_command_bar(cx) {
            Some(bar) => v_resizable(SharedString::from(format!("cmd-split-{}", self.active)))
                .child(
                    resizable_panel()
                        .size(px(150.))
                        .size_range(px(84.)..px(560.))
                        .child(bar),
                )
                .child(resizable_panel().child(rest))
                .into_any_element(),
            None => rest.into_any_element(),
        };
        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().child(main))
            .into_any_element()
    }

    /// The one bar at the bottom: facts about the active tab on the left, paging and
    /// export on the right (docs/spec/workspace.md §5).
    fn render_status(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let table = match self.tabs.get(self.active) {
            Some(Tab::Table(t)) if t.view == TableView::Data && self.session.is_some() => Some(t),
            _ => None,
        };

        // Keep the page box showing the current page unless the user is typing in it.
        if let Some(t) = table {
            if let Some(p) = &t.page {
                let shown = (p.page + 1).to_string();
                let input = t.page_input.clone();
                if !input.read(cx).focus_handle(cx).is_focused(window)
                    && input.read(cx).value() != shown.as_str()
                {
                    input.update(cx, |s, cx| s.set_value(shown, window, cx));
                }
            }
        }

        // Row count (with timing) and how long ago the page arrived.
        let facts: Option<(String, Option<String>)> = any_table_facts(self, cx);
        let (msg, err) = match self.status.clone() {
            Some((m, e)) => (Some(m), e),
            None => (None, false),
        };

        // The bar takes the connection's colour, so it is clear which server this is.
        let conn_color = self
            .session
            .as_ref()
            .and_then(|s| s.color.as_deref())
            .and_then(parse_hex);
        let status_bg = conn_color.unwrap_or(theme.status_bar);
        let status_border = conn_color.unwrap_or(theme.status_bar_border);
        let status_ink = conn_color.map_or(theme.muted_foreground, ink_on);
        // On a coloured bar every control takes the same ink as the text.
        let bar_ink = conn_color.map(|_| status_ink);
        let ghost = bar_ink.map(|i| bar_button(i, cx));
        // Three parts, like the window above it: the connection under the sidebar,
        // facts about the tab in the middle, the tab's controls on the right.
        let ink = bar_ink.unwrap_or(theme.muted_foreground);
        let sidebar_w = self
            .main_split
            .read(cx)
            .sizes()
            .first()
            .copied()
            .unwrap_or(px(270.));
        let left = h_flex()
            .w(sidebar_w)
            .flex_none()
            .h_full()
            .px_3()
            .gap_2()
            .border_r_1()
            .border_color(ink.opacity(0.2))
            .map(|d| match &self.session {
                Some(s) => {
                    let db = self
                        .tree
                        .current_database()
                        .map(str::to_string)
                        .unwrap_or_else(|| s.name.clone());
                    let driver = self
                        .drivers
                        .iter()
                        .find(|x| x.id == s.driver)
                        .map_or(s.driver.clone(), |x| x.name.clone());
                    d.child(db_icon(&s.driver).size_3p5().flex_none().text_color(ink))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(db),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(ink.opacity(0.75))
                                .child(driver),
                        )
                }
                None => d.child(div().text_color(ink.opacity(0.75)).child("Not connected")),
            });
        let mut bar = h_flex()
            .h(px(36.))
            .flex_none()
            .text_xs()
            .text_color(status_ink)
            .bg(status_bg)
            .border_t_1()
            .border_color(status_border)
            .child(left);

        // The open table: a switch to its other view, then its row count and age.
        let any_table = match self.tabs.get(self.active) {
            Some(Tab::Table(t)) if !self.on_home && self.session.is_some() => Some(t),
            _ => None,
        };
        let mut middle = h_flex().flex_1().min_w_0().h_full().px_3().gap_4();
        if let Some(t) = any_table {
            let to_structure = t.view == TableView::Data;
            let object = t.object.clone();
            middle = middle.when(!self.is_mongo(), |m| {
                m.child(
                    bar_chip(
                        "view-switch",
                        if to_structure { "Structure" } else { "Data" },
                        if to_structure { "list" } else { "table" },
                        ink,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if to_structure {
                            this.show_structure(&object, cx);
                        } else if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                            t.view = TableView::Data;
                            cx.notify();
                        }
                    })),
                )
            });
        }
        if err {
            middle = middle.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(conn_color.is_none(), |d| d.text_color(theme.danger))
                    .when(conn_color.is_some(), |d| d.font_weight(FontWeight::MEDIUM))
                    .child(msg.unwrap_or_default()),
            );
        } else {
            let fact = |ic: &'static str, text: String| {
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .child(icon(ic).size_3p5().text_color(ink.opacity(0.75)))
                    .child(div().font_family(mono.clone()).child(text))
            };
            middle = middle
                .when_some(facts, |d, (count, age)| {
                    d.child(fact("hash", count))
                        .when_some(age, |d, a| d.child(fact("clock", a)))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when_some(msg, |d, m| d.child(m)),
                );
        }
        bar = bar.child(middle);

        let Some(t) = table else {
            return bar.into_any_element();
        };
        let Some((page, pages)) = t.page.as_ref().map(|p| (p.page, p.page_count())) else {
            return bar.into_any_element();
        };
        let mongo = self.is_mongo();
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let doc_selected = mongo
            && t.view == TableView::Data
            && self
                .selected_doc_ref(cx)
                .is_some_and(|d| d.get("_id").is_some());
        // While a number box has the cursor it gets an ink ring and a stronger well.
        let focused = |input: &Entity<InputState>, cx: &App| {
            input.read(cx).focus_handle(cx).is_focused(window)
        };
        let size_focused = focused(&t.page_size_input, cx);
        let page_focused = focused(&t.page_input, cx);
        let size_box = self.page_size_box(ink, bar_ink, size_focused, cx);
        let square = |id: &'static str, ic: &'static str| {
            Button::new(id)
                .xsmall()
                .custom(bar_square(ink, cx))
                .icon(icon(ic))
                .w(px(28.))
                .h(px(24.))
        };
        let weak = cx.entity().downgrade();
        let view_sql = self.view_as_query(cx);
        let settings = square("table-menu", "settings").dropdown_menu_with_anchor(
            Anchor::BottomRight,
            move |menu, window, cx| {
                let sql = view_sql.clone();
                let (w1, w2) = (weak.clone(), weak.clone());
                menu.submenu_with_icon(
                    Some(icon("download")),
                    "Export whole table",
                    window,
                    cx,
                    move |m, _, _| {
                        m.item(app_item(&w1, "CSV file", |a, cx| {
                            a.export_table_to_file(ExportFormat::Csv, cx)
                        }))
                        .item(app_item(&w1, "JSON file", |a, cx| {
                            a.export_table_to_file(ExportFormat::Json, cx)
                        }))
                    },
                )
                .submenu_with_icon(
                    Some(icon("clipboard")),
                    "Copy this page",
                    window,
                    cx,
                    move |m, _, _| {
                        m.item(app_item(&w2, "CSV", |a, cx| {
                            a.copy_result(ExportFormat::Csv, cx)
                        }))
                        .item(app_item(&w2, "JSON", |a, cx| {
                            a.copy_result(ExportFormat::Json, cx)
                        }))
                        .item(app_item(&w2, "Markdown", |a, cx| {
                            a.copy_result(ExportFormat::Markdown, cx)
                        }))
                        .item(app_item(&w2, "SQL INSERT", |a, cx| {
                            a.copy_result(ExportFormat::SqlInsert, cx)
                        }))
                    },
                )
                .item(
                    PopupMenuItem::new(if mongo {
                        "Copy view as find()"
                    } else {
                        "Copy view as SQL"
                    })
                    .icon(icon("code"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(sql.clone()))
                    }),
                )
            },
        );

        bar.child(
            h_flex()
                .flex_none()
                .h_full()
                .pr_3()
                .gap_1()
                .child(size_box)
                .child(bar_divider(bar_ink, &theme))
                .child(
                    Button::new("prev")
                        .ghost()
                        .xsmall()
                        .when_some(ghost, |b, v| b.custom(v))
                        .icon(icon("chevron-left"))
                        .tooltip("Previous page")
                        .disabled(page == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                t.page_ix = t.page_ix.saturating_sub(1);
                            }
                            this.load_page(cx);
                        })),
                )
                .child(
                    // A flat well instead of a boxed field, so it sits in the bar.
                    div()
                        .w(px(40.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if page_focused {
                            ink
                        } else {
                            transparent_black()
                        })
                        .bg(ink.opacity(if page_focused { 0.2 } else { 0.12 }))
                        .child(
                            Input::new(&t.page_input)
                                .xsmall()
                                .appearance(false)
                                .text_center()
                                .font_family(mono.clone())
                                .when_some(bar_ink, |d, i| d.text_color(i)),
                        ),
                )
                .child(
                    div()
                        .pl_1()
                        .font_family(mono)
                        .text_color(ink.opacity(0.75))
                        .child(format!("of {pages}")),
                )
                .child(
                    Button::new("next")
                        .ghost()
                        .xsmall()
                        .when_some(ghost, |b, v| b.custom(v))
                        .icon(icon("chevron-right"))
                        .tooltip("Next page")
                        .disabled(page + 1 >= pages)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                t.page_ix += 1;
                            }
                            this.load_page(cx);
                        })),
                )
                .child(bar_divider(bar_ink, &theme))
                .child(
                    square("refresh-page", "refresh-cw")
                        .tooltip("Fetch this page again")
                        .on_click(cx.listener(|this, _, _, cx| this.load_page(cx))),
                )
                .when(!read_only, |d| {
                    d.child(
                        square("add-row", "plus")
                            .tooltip_with_action(
                                if mongo { "Insert document" } else { "Add row" },
                                &AddRow,
                                Some("ThaanApp"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.add_row(window, cx))),
                    )
                })
                // MongoDB: the selected document can be edited (or viewed, read-only)
                // and deleted from here; both open a dialog first.
                .when(doc_selected, |d| {
                    d.child(
                        square("edit-doc", if read_only { "eye" } else { "pencil" })
                            .tooltip(if read_only {
                                "View document"
                            } else {
                                "Edit document"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let kind = if read_only {
                                    doc_editor::DocDialogKind::View
                                } else {
                                    doc_editor::DocDialogKind::Edit
                                };
                                this.open_doc_dialog(kind, window, cx)
                            })),
                    )
                    .when(!read_only, |d| {
                        d.child(
                            square("delete-doc", "trash")
                                .tooltip("Delete document")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_delete_selected_doc(window, cx)
                                })),
                        )
                    })
                })
                .child(settings)
                .children(self.render_staged_actions(bar_ink.map(|i| (i, status_bg)), cx)),
        )
        .into_any_element()
    }

    /// The open table tab's current view (filters, sort, page) as text to paste into a
    /// query: a SELECT with the values inlined, or a find() on MongoDB.
    fn view_as_query(&self, cx: &App) -> String {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return String::new();
        };
        let driver = self.session.as_ref().map_or("", |s| s.driver.as_str());
        let (filters, raw_where) = t.where_parts(cx);
        let (projection, raw_sort) = t.document_shape(cx);
        if driver == "mongodb" {
            let n = thaan_core::doc::Doc::String(t.object.name.clone()).to_pretty();
            let filter = t.filter.read(cx).value().trim().to_string();
            let mut q = format!(
                "db.getCollection({n}).find({}{})",
                if filter.is_empty() { "{}" } else { &filter },
                projection.map(|p| format!(", {p}")).unwrap_or_default()
            );
            if let Some(s) = raw_sort {
                q.push_str(&format!(".sort({s})"));
            }
            q.push_str(&format!(
                ".skip({}).limit({})",
                t.page_ix * t.page_size,
                t.page_size
            ));
            return q;
        }
        if matches!(driver, "redis" | "mqtt") {
            // Line-based languages: the key or topic's own command (no SQL to show).
            return command_bar::starter_command(driver, &t.object);
        }
        let d = thaan_core::dialect::Dialect::for_driver(driver);
        let req = PageRequest {
            table: t.object.clone(),
            filters,
            raw_where,
            projection: None,
            raw_sort: None,
            sort: t.sort.clone(),
            page: t.page_ix,
            page_size: t.page_size,
        };
        thaan_core::table::page_queries(&d, &req).0.preview(&d)
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (items, _) = self.palette_layout(cx);
        let sel = self.palette_sel.min(items.len().saturating_sub(1));
        let searching = !self.palette_input.read(cx).value().trim().is_empty();

        let mut list = v_flex()
            .id("palette-list")
            .track_scroll(&self.palette_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2();
        let mut last_cat = "";
        for (i, c) in items.iter().enumerate() {
            if c.category != last_cat {
                last_cat = c.category;
                list = list.child(
                    group_label(c.category, None, &theme)
                        .flex_none()
                        .px_2()
                        .pt(px(if i == 0 { 4. } else { 12. }))
                        .pb_1(),
                );
            }
            let id = c.id;
            let runs_here = gui_runs(id);
            list = list.child(
                h_flex()
                    .id(("cmd", i))
                    .flex_none()
                    .h(px(32.))
                    .px_2()
                    .gap_3()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(i == sel, |d| d.bg(theme.list_hover))
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(!runs_here, |d| d.text_color(theme.muted_foreground))
                            .child(c.title),
                    )
                    .when(!runs_here, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child("CLI"),
                        )
                    })
                    .children(c.shortcut.and_then(kbd))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_palette_command(id, window, cx)
                    })),
            );
        }
        if items.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No matching commands"),
            );
        }

        let hint = |keys: &[&'static str], label: &'static str| {
            h_flex()
                .gap_1()
                .children(keys.iter().filter_map(|k| kbd(k)))
                .child(div().pl_0p5().child(label))
        };
        let footer = h_flex()
            .h(px(36.))
            .flex_none()
            .px_4()
            .gap_4()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(hint(&["up", "down"], "Move"))
            .child(hint(&["enter"], "Run"))
            .child(hint(&["escape"], "Close"))
            .child(div().flex_1())
            .when(!searching, |d| d.child("Type to search CLI commands too"));

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .bg(theme.overlay)
            .child(
                v_flex()
                    .id("palette")
                    .w(px(560.))
                    .h(px(440.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .overflow_hidden()
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "up" => this.move_palette_selection(-1, cx),
                            "down" => this.move_palette_selection(1, cx),
                            "escape" => {
                                this.palette_open = false;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .p_2()
                            .child(Input::new(&self.palette_input).prefix(icon("search").size_4())),
                    )
                    .child(list)
                    .child(footer),
            )
    }
}

impl Render for ThaanApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let root = v_flex()
            .key_context("ThaanApp")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &RunCurrent, _, cx| this.run(true, cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run(false, cx)))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                if this.session.is_some() {
                    this.new_query_tab(None, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_tab(this.active, cx)))
            .on_action(cx.listener(|this, _: &RefreshSchema, _, cx| this.refresh_schema(cx)))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &AddRow, window, cx| this.add_row(window, cx)))
            .on_action(cx.listener(|this, _: &PasteCell, window, cx| {
                if let Some((row, col)) = this.selected_cell(cx) {
                    this.grid_action(crate::grid::GridAction::Paste { row, col }, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CloneRow, window, cx| {
                if let Some((row, _)) = this.selected_cell(cx) {
                    this.grid_action(crate::grid::GridAction::CloneRow { row }, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteRow, window, cx| {
                if let Some((row, _)) = this.selected_cell(cx) {
                    this.grid_action(crate::grid::GridAction::DeleteRow { row }, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Disconnect, _, cx| this.disconnect(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TogglePalette, window, cx| this.toggle_palette(window, cx)),
            );

        // The title bar always names the product. The open connection is named by its
        // tab, its sidebar row and the status bar; only read-only is repeated here, as a
        // safety cue.
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let title = h_flex()
            .gap_2()
            .items_center()
            .child(crate::logo::mark(theme.primary, theme.foreground))
            .child(div().font_medium().child("ThaanDB"))
            .when(read_only, |d| {
                d.child(
                    h_flex()
                        .gap_1()
                        .text_color(theme.muted_foreground)
                        .child(icon("lock").size_3p5())
                        .child("Read-only"),
                )
            });
        // One row: macOS traffic lights (the kit's TitleBar leaves room for them), then
        // the ThaanDB logo and name, New connection and the command palette.
        let header = TitleBar::new().child(
            h_flex()
                .flex_1()
                .h_full()
                .pr_3()
                .gap_3()
                .text_sm()
                .child(title)
                .child(div().w(px(1.)).h(px(16.)).bg(theme.border))
                .child(
                    Button::new("title-new-conn")
                        .ghost()
                        .small()
                        .icon(icon("database-plus"))
                        .label("New connection")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_conn_dialog(window, cx)),
                        ),
                )
                .child(div().flex_1())
                .child(
                    h_flex()
                        .id("palette")
                        .h(px(24.))
                        .px_2()
                        .gap_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .hover(|s| s.bg(theme.list_hover))
                        .child(div().pr_1().child("Commands"))
                        .children(kbd(PALETTE_KEY))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_palette(window, cx)),
                        ),
                )
                .child(
                    Button::new("title-settings")
                        .ghost()
                        .small()
                        .icon(icon("settings"))
                        .tooltip_with_action("Settings", &OpenSettings, Some("ThaanApp"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                        ),
                ),
        );

        let status = self.render_status(window, cx);

        // One page, connected or not: the sidebar tree and the tab strip, where Home is
        // the first tab. Connecting only changes that connection's row in the tree.
        let main: AnyElement = {
            let content = if self.session.is_none() || self.on_home || self.tabs.is_empty() {
                // The width Home gets: the window less the rail and the sidebar.
                let sidebar_w = self
                    .main_split
                    .read(cx)
                    .sizes()
                    .first()
                    .copied()
                    .unwrap_or(px(270.));
                let width = window.viewport_size().width - sidebar_w - px(40.);
                self.render_home(false, width, cx)
            } else if matches!(self.tabs.get(self.active), Some(Tab::Query(_))) {
                self.render_query_tab(cx)
            } else if let Some(Tab::View(v)) = self.tabs.get(self.active) {
                v.view.clone().into_any_element()
            } else {
                self.render_table_tab(cx)
            };
            h_resizable("main-split")
                .with_state(&self.main_split)
                .child(
                    resizable_panel()
                        .size(px(270.))
                        .size_range(px(180.)..px(520.))
                        .child(self.render_sidebar(cx)),
                )
                .child(
                    resizable_panel().child(
                        v_flex()
                            .size_full()
                            .child(self.render_tabs(cx))
                            .child(div().flex_1().overflow_hidden().child(content)),
                    ),
                )
                .into_any_element()
        };

        root.child(header)
            .child(div().flex_1().overflow_hidden().child(main))
            .child(status)
            .when(self.palette_open, |d| d.child(self.render_palette(cx)))
    }
}

// ---- small view helpers ---------------------------------------------------------------

type ThemeRef = gpui_kit::component::Theme;

/// Rows per page when a table opens.
/// Page sizes offered in the status bar's list; any other can be typed.
const PAGE_SIZES: [u64; 7] = [1, 5, 10, 25, 50, 100, 200];
/// The largest page size that can be typed, so one page never loads an unbounded result.
const MAX_PAGE_SIZE: u64 = 1000;

/// Height of every one-line row in the sidebar, the palette and the saved list.
const ROW_H: f32 = 28.;

fn field(label: &str, input: AnyElement, theme: &ThemeRef) -> impl IntoElement {
    v_flex()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(theme.secondary_foreground)
                .child(label.to_string()),
        )
        .child(input)
}

/// A group label in sentence case with an optional count in muted mono, e.g. `Tables 2`.
fn group_label(title: &str, count: Option<usize>, theme: &ThemeRef) -> Div {
    h_flex()
        .gap_1p5()
        .items_baseline()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(div().font_medium().child(title.to_string()))
        .when_some(count, |d, n| {
            d.child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child(n.to_string()),
            )
        })
}

/// A key binding in GPUI keystroke syntax (`cmd-k`), drawn with the kit's `Kbd`.
fn kbd(binding: &'static str) -> Option<Kbd> {
    Keystroke::parse(binding).ok().map(Kbd::new)
}

/// The command palette binding for this platform.
const PALETTE_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-k"
} else {
    "ctrl-k"
};
/// New query tab and run-statement bindings, for the hints on home.
const NEW_TAB_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-t"
} else {
    "ctrl-t"
};
const RUN_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

/// The connection's own colour as a dot: the one signature mark, drawn the same everywhere.
/// `#rrggbb` for a picked colour (stored with the connection).
/// Black or white, whichever reads on a solid fill of `c`.
pub(crate) fn ink_on(c: Hsla) -> Hsla {
    let r = Rgba::from(c);
    let lum = 0.2126 * r.r + 0.7152 * r.g + 0.0722 * r.b;
    if lum > 0.5 {
        gpui_kit::black()
    } else {
        gpui_kit::white()
    }
}

/// A ghost status-bar button drawn in `ink`, with a faint ink hover.
pub(crate) fn bar_button(ink: Hsla, cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(transparent_black())
        .foreground(ink)
        .hover(ink.opacity(0.12))
        .active(ink.opacity(0.2))
}

/// A button drawn for the coloured status bar. The kit draws custom-coloured buttons at
/// 20% strength, which leaves a primary action looking disabled there, so these two are
/// our own: `solid` fills with the bar's ink and writes in the bar's colour; otherwise an
/// ink outline. 24px tall, 6px corners, like the kit's xsmall buttons.
pub(crate) fn bar_pill(
    id: &'static str,
    label: &'static str,
    icon_name: Option<&'static str>,
    solid: bool,
    ink: Hsla,
    bar: Hsla,
) -> Stateful<Div> {
    let fg = if solid { bar } else { ink };
    h_flex()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px_3()
        .gap_1p5()
        .rounded(px(6.))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .cursor_pointer()
        .when(solid, |d| {
            d.bg(ink)
                .hover(move |s| s.bg(ink.opacity(0.85)))
                .active(move |s| s.bg(ink.opacity(0.7)))
        })
        .when(!solid, |d| {
            d.border_1()
                .border_color(ink.opacity(0.55))
                .hover(move |s| s.bg(ink.opacity(0.12)))
                .active(move |s| s.bg(ink.opacity(0.2)))
        })
        .when_some(icon_name, |d, n| d.child(icon(n).size_3p5().text_color(fg)))
        .child(label)
}

/// A hairline between groups in the status bar.
pub(crate) fn bar_divider(ink: Option<Hsla>, theme: &ThemeRef) -> impl IntoElement {
    div()
        .mx_2()
        .w(px(1.))
        .h(px(14.))
        .bg(ink.map_or(theme.border, |i| i.opacity(0.25)))
}

fn hex_color(c: Hsla) -> String {
    let r = Rgba::from(c);
    let b = |x: f32| (x.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(r.r), b(r.g), b(r.b))
}

fn conn_dot(color: Hsla) -> impl IntoElement {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn struct_row(cells: [String; 6], head: bool, theme: &ThemeRef) -> impl IntoElement {
    let widths = [40., 180., 140., 80., 160., 80.];
    h_flex()
        .h(px(32.))
        .text_sm()
        .border_b_1()
        .border_color(theme.border)
        .when(head, |d| {
            d.bg(theme.table_head)
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
        })
        .children(
            cells
                .into_iter()
                .zip(widths)
                .enumerate()
                .map(|(i, (c, w))| {
                    div()
                        .w(px(w))
                        .px_3()
                        .truncate()
                        .when(i == 0, |d| {
                            d.font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                        })
                        .child(c)
                }),
        )
}

fn list_panel(title: &str, count: usize, hint: &str, theme: &ThemeRef) -> Stateful<Div> {
    v_flex()
        .id(SharedString::from(format!("panel-{title}")))
        .size_full()
        .overflow_y_scroll()
        .px_2()
        .pb_2()
        .gap_0p5()
        .child(
            group_label(title, (count > 0).then_some(count), theme)
                .px_2()
                .pt_3()
                .pb_1(),
        )
        .when(count == 0, |d| {
            d.child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(hint.to_string()),
            )
        })
}

fn empty_state(text: &str, theme: &ThemeRef) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim_start_matches('#');
    u32::from_str_radix(h, 16)
        .ok()
        .filter(|_| h.len() == 6)
        .map(|v| rgb(v).into())
}

/// The open table tab's facts for the status bar: its row count (with timing) and how
/// long ago its page arrived.
fn any_table_facts(app: &ThaanApp, _cx: &App) -> Option<(String, Option<String>)> {
    let Some(Tab::Table(t)) = app.tabs.get(app.active) else {
        return None;
    };
    if app.on_home {
        return None;
    }
    if let Some(rs) = t.command_result.as_ref().and_then(|r| r.result.as_ref()) {
        return Some((
            format!(
                "Query · {} · {} ms",
                plural(rs.rows.len() as u64, "row"),
                rs.elapsed_ms
            ),
            None,
        ));
    }
    let p = t.page.as_ref()?;
    let ago = t.fetched_at.map(|at| {
        let s = at.elapsed().as_secs();
        match s {
            0..=29 => "just now".to_string(),
            30..=89 => "1 min ago".to_string(),
            _ if s < 3600 => format!("{} min ago", (s + 30) / 60),
            _ => format!("{} h ago", s / 3600),
        }
    });
    let count = if p.result.documents.is_some() {
        // MongoDB wording (docs/spec/mongodb-ui.md M7).
        let first = p.page * p.page_size;
        let shown = p.result.rows.len() as u64;
        if shown == 0 {
            format!("No documents · {} ms", p.result.elapsed_ms)
        } else {
            format!(
                "Documents {}–{} of {} · {} ms",
                first + 1,
                first + shown,
                p.total_rows,
                p.result.elapsed_ms
            )
        }
    } else {
        format!(
            "{} · {} ms",
            plural(p.total_rows, "row"),
            p.result.elapsed_ms
        )
    };
    Some((count, ago))
}

/// A filled square status-bar button: the bar's ink at about 12% behind an ink icon.
/// The kit draws a custom fill at 20% strength, so 0.6 here lands near 12%.
pub(crate) fn bar_square(ink: Hsla, cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(ink.opacity(0.6))
        .foreground(ink)
        .hover(ink.opacity(0.2))
        .active(ink.opacity(0.28))
}

/// A small labelled status-bar control (the Structure / Data switch): icon and label in
/// ink on a 12% ink well, 24px tall.
pub(crate) fn bar_chip(
    id: &'static str,
    label: &'static str,
    ic: &'static str,
    ink: Hsla,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px_2()
        .gap_1p5()
        .rounded(px(6.))
        .bg(ink.opacity(0.12))
        .hover(move |s| s.bg(ink.opacity(0.2)))
        .active(move |s| s.bg(ink.opacity(0.28)))
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink)
        .child(icon(ic).size_3p5().text_color(ink))
        .child(label)
}

/// A menu row that runs `f` on the app.
fn app_item(
    weak: &WeakEntity<ThaanApp>,
    label: &'static str,
    f: fn(&mut ThaanApp, &mut Context<ThaanApp>),
) -> PopupMenuItem {
    let weak = weak.clone();
    PopupMenuItem::new(label).on_click(move |_, _, cx| {
        weak.update(cx, f).ok();
    })
}

impl ThaanApp {
    /// Sets the open table's rows per page (typed or picked), back to page 1, and
    /// reloads. An empty or invalid entry puts the current size back in the box.
    fn set_page_size(&mut self, size: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) else {
            return;
        };
        let size = size.filter(|n| *n > 0).map(|n| n.min(MAX_PAGE_SIZE));
        let changed = size.is_some_and(|n| n != t.page_size);
        if let Some(n) = size {
            t.page_size = n;
        }
        let shown = t.page_size.to_string();
        t.page_size_input
            .update(cx, |s, cx| s.set_value(shown, window, cx));
        if changed {
            t.page_ix = 0;
            self.load_page(cx);
        }
    }

    /// Rows per page: a typeable box with a list of common sizes beside it.
    fn page_size_box(
        &self,
        ink: Hsla,
        bar_ink: Option<Hsla>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return div().into_any_element();
        };
        let mono = cx.theme().mono_font_family.clone();
        let current = t.page_size;
        let weak = cx.entity().downgrade();
        let list_mono = mono.clone();
        // "Rows" then one compact well: the typeable number and its list arrow read as
        // a single control, sized like the page box next to it.
        h_flex()
            .flex_none()
            .gap_1p5()
            .child(div().text_color(ink.opacity(0.75)).child("Rows"))
            .child(
                h_flex()
                    .h(px(24.))
                    .pr_0p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if focused { ink } else { transparent_black() })
                    .bg(ink.opacity(if focused { 0.2 } else { 0.12 }))
                    .child(
                        div().w(px(36.)).child(
                            Input::new(&t.page_size_input)
                                .xsmall()
                                .appearance(false)
                                .text_center()
                                .font_family(mono)
                                .when_some(bar_ink, |d, i| d.text_color(i)),
                        ),
                    )
                    .child(
                        Button::new("page-size")
                            .ghost()
                            .xsmall()
                            .custom(bar_button(ink, cx))
                            .icon(icon("chevron-down").size_3())
                            .w(px(20.))
                            .h(px(20.))
                            .tooltip("Rows per page")
                            .dropdown_menu_with_anchor(
                                Anchor::BottomRight,
                                move |mut menu, _, _| {
                                    // A short list of numbers: narrow, no heading, the check on
                                    // the right so the figures line up on the left.
                                    menu = menu
                                        .min_w(px(80.))
                                        .check_side(gpui_kit::component::Side::Right);
                                    for n in PAGE_SIZES {
                                        let weak = weak.clone();
                                        let mono = list_mono.clone();
                                        menu = menu.item(
                                            PopupMenuItem::element(move |_, _| {
                                                div().font_family(mono.clone()).child(n.to_string())
                                            })
                                            .checked(n == current)
                                            .on_click(move |_, window, cx| {
                                                weak.update(cx, |app, cx| {
                                                    app.set_page_size(Some(n), window, cx)
                                                })
                                                .ok();
                                            }),
                                        );
                                    }
                                    menu
                                },
                            ),
                    ),
            )
            .into_any_element()
    }
}
