//! The object tree in the sidebar: every saved connection (in its groups), and under each
//! open one its databases → (schemas) → folders → objects → their indexes and triggers
//! (docs/spec/workspace.md, sidebar).
//!
//! Several connections can be open at once (see `live.rs`). An open connection shows a
//! green dot and its logo in its own colour; the others stay muted until clicked, which
//! connects them without closing anything. Other databases of a server are listed on
//! demand through `Engine::list_objects_in`; opening an object in another database
//! switches that session to it first. A child module of `app`.

use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use std::collections::{HashMap, HashSet};
use thaan_core::engine::SessionId;

#[derive(Default)]
pub(super) struct ObjectTree {
    databases: Vec<String>,
    current: Option<String>,
    expanded: HashSet<String>,
    /// Objects of databases other than the current one, once loaded.
    other: HashMap<String, Vec<SchemaObject>>,
    loading: HashSet<String>,
    errors: HashMap<String, String>,
    /// Extra sessions on other databases of this server, opened when a table there is
    /// opened. They close with the connection.
    db_sessions: HashMap<String, SessionId>,
    /// Databases whose extra session is being opened.
    opening: HashSet<String>,
}

/// What a click on a row (or its chevron) does.
#[derive(Clone)]
enum RowAction {
    None,
    /// Fold or unfold a node of one session's tree.
    Toggle(SessionId, String),
    /// Expand a database that is not loaded yet.
    LoadDatabase(SessionId, String),
    Open {
        sid: SessionId,
        database: Option<String>,
        object: ObjectRef,
    },
    /// A saved connection that is not open yet.
    Connect(Box<ConnectionConfig>),
    /// An open connection: bring it to the front (and unfold it).
    Focus(SessionId),
    /// Fold or unfold a connection group.
    Group(String),
    /// Redis: the key browser of one database.
    OpenKeys(SessionId, String),
    /// MQTT: the connection's console.
    OpenConsole(SessionId),
}

#[derive(Clone)]
enum RowIcon {
    Lucide(&'static str),
    /// A database logo, by driver id.
    Db(String),
}

struct Row {
    /// Unique across the whole list (used for element ids).
    id: String,
    depth: usize,
    icon: RowIcon,
    /// Draws the icon in this colour instead of muted (open connections and databases).
    tint: Option<Hsla>,
    label: String,
    /// Muted text after the label (a count, a state).
    meta: Option<String>,
    /// `Some(open)` for rows with children.
    expander: Option<bool>,
    strong: bool,
    /// The connection in front.
    selected: bool,
    /// A green dot: this connection or database is connected.
    live: bool,
    action: RowAction,
    /// What the chevron does; `None` means the same as `action`.
    chevron: Option<RowAction>,
    /// Right-click menu for connection rows.
    menu: Option<ConnMenu>,
}

#[derive(Clone)]
enum ConnMenu {
    /// An open connection, with its saved settings when it has them (for Copy).
    Open(SessionId, Option<Box<ConnectionConfig>>),
    Closed(Box<ConnectionConfig>),
    /// A table, view or collection of an open connection.
    Object(Box<ObjectMenu>),
    /// A database of an open connection.
    Database(DatabaseMenu),
}

/// What to do once a database's session is ready.
enum DbThen {
    OpenTable(ObjectRef),
    NewQuery,
    /// The Redis key browser of that database.
    OpenKeys,
    /// A query tab that starts with this text (New table's CREATE TABLE).
    NewQueryWith(String),
}

/// The session a tab reads from or runs on.
fn tab_session(t: &Tab) -> SessionId {
    match t {
        Tab::Query(q) => q.sid,
        Tab::Table(t) => t.sid,
        Tab::View(v) => v.sid,
    }
}

/// What the right-click menu of a database row needs.
#[derive(Clone)]
struct DatabaseMenu {
    ws: SessionId,
    database: String,
    /// The connection's own database (its session cannot be closed on its own).
    current: bool,
    /// A second session is open for it.
    open: bool,
    driver: String,
}

/// What the right-click menu of an object row needs.
#[derive(Clone)]
struct ObjectMenu {
    sid: SessionId,
    /// Set for an object of another database than the session's own.
    database: Option<String>,
    object: ObjectRef,
    kind: ObjectKind,
    driver: String,
}

impl ObjectMenu {
    /// `sid` is the connection's own session for these rows (see `object_rows`).
    fn target(&self) -> super::table_ops::OpTarget {
        super::table_ops::OpTarget {
            ws: self.sid,
            database: self.database.clone(),
            object: self.object.clone(),
            kind: self.kind,
            driver: self.driver.clone(),
        }
    }
}

impl Row {
    fn new(id: String, depth: usize, icon: RowIcon, label: String, action: RowAction) -> Self {
        Row {
            id,
            depth,
            icon,
            tint: None,
            label,
            meta: None,
            expander: None,
            strong: false,
            selected: false,
            live: false,
            action,
            chevron: None,
            menu: None,
        }
    }
}

/// One open session's data, borrowed for building its rows.
struct Live<'a> {
    session: &'a SessionInfo,
    tree: &'a ObjectTree,
    objects: &'a [SchemaObject],
    selected: bool,
}

fn folder_label(kind: ObjectKind, mongo: bool) -> &'static str {
    if mongo && kind == ObjectKind::Table {
        "Collections"
    } else {
        kind.label_plural()
    }
}

fn kind_icon(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Table => "table",
        ObjectKind::View | ObjectKind::MaterializedView => "eye",
        ObjectKind::Index => "list",
        ObjectKind::Trigger => "zap",
        ObjectKind::Sequence => "hash",
        ObjectKind::Routine => "square-function",
    }
}

/// Kinds that get a folder of their own; indexes and triggers sit under their table.
const FOLDERS: [ObjectKind; 5] = [
    ObjectKind::Table,
    ObjectKind::View,
    ObjectKind::MaterializedView,
    ObjectKind::Routine,
    ObjectKind::Sequence,
];

impl ObjectTree {
    fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
    }

    fn open(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }

    /// The database the connection's own session is on.
    pub(super) fn current_database(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// The second session opened for `database`, if any.
    pub(super) fn db_session(&self, database: &str) -> Option<SessionId> {
        self.db_sessions.get(database).copied()
    }

    pub(super) fn remember_db_session(&mut self, database: &str, sid: SessionId) {
        self.db_sessions.insert(database.to_string(), sid);
    }

    /// Every extra database session, to close with the connection.
    pub(super) fn extra_sessions(&self) -> Vec<SessionId> {
        self.db_sessions.values().copied().collect()
    }
}

/// Rows under one database (or one schema of it).
fn object_rows(
    l: &Live,
    rows: &mut Vec<Row>,
    prefix: &str,
    depth: usize,
    database: Option<&str>,
    objects: &[&SchemaObject],
    filter: &str,
) {
    let sid = l.session.session_id;
    let tree = l.tree;
    let mongo = l.session.driver == "mongodb";
    let rid = |key: &str| format!("{sid}:{key}");
    let other_db = database
        .filter(|d| Some(*d) != tree.current.as_deref())
        .map(str::to_string);
    let matches =
        |o: &SchemaObject| filter.is_empty() || o.object.name.to_lowercase().contains(filter);
    for kind in FOLDERS {
        let items: Vec<&SchemaObject> = objects
            .iter()
            .copied()
            .filter(|o| o.kind == kind && matches(o))
            .collect();
        // Tables and views always show (empty folders say "nothing here"); the rest
        // only when they have something.
        let always = matches!(kind, ObjectKind::Table | ObjectKind::View) && filter.is_empty();
        if items.is_empty() && !always {
            continue;
        }
        let key = format!("{prefix}/f:{kind:?}");
        let open = tree.open(&key) || !filter.is_empty();
        let mut row = Row::new(
            rid(&key),
            depth,
            RowIcon::Lucide(if open { "folder-open" } else { "folder" }),
            folder_label(kind, mongo).to_string(),
            RowAction::Toggle(sid, key.clone()),
        );
        row.meta = Some(items.len().to_string());
        row.expander = Some(open);
        rows.push(row);
        if !open {
            continue;
        }
        for o in items {
            // Indexes and triggers of this table (same schema).
            let children: Vec<&SchemaObject> = objects
                .iter()
                .copied()
                .filter(|c| {
                    matches!(c.kind, ObjectKind::Index | ObjectKind::Trigger)
                        && c.parent.as_deref() == Some(o.object.name.as_str())
                        && c.object.schema == o.object.schema
                })
                .collect();
            let okey = format!("{key}/o:{}", o.object.name);
            let oopen = tree.open(&okey);
            let mut row = Row::new(
                rid(&okey),
                depth + 1,
                RowIcon::Lucide(kind_icon(kind)),
                o.object.name.clone(),
                RowAction::Open {
                    sid,
                    database: other_db.clone(),
                    object: o.object.clone(),
                },
            );
            row.expander = (!children.is_empty()).then_some(oopen);
            row.menu = Some(ConnMenu::Object(Box::new(ObjectMenu {
                sid,
                database: other_db.clone(),
                object: o.object.clone(),
                kind,
                driver: l.session.driver.clone(),
            })));
            row.chevron = Some(RowAction::Toggle(sid, okey.clone()));
            rows.push(row);
            if oopen {
                for c in children {
                    let mut row = Row::new(
                        rid(&format!("{okey}/c:{:?}:{}", c.kind, c.object.name)),
                        depth + 2,
                        RowIcon::Lucide(kind_icon(c.kind)),
                        c.object.name.clone(),
                        RowAction::None,
                    );
                    row.meta = Some(
                        if c.kind == ObjectKind::Index {
                            "index"
                        } else {
                            "trigger"
                        }
                        .into(),
                    );
                    rows.push(row);
                }
            }
        }
    }
    // Indexes or triggers whose table is not listed (rare) still show somewhere.
    let orphans: Vec<&SchemaObject> = objects
        .iter()
        .copied()
        .filter(|c| {
            matches!(c.kind, ObjectKind::Index | ObjectKind::Trigger)
                && matches(c)
                && !objects.iter().any(|o| {
                    o.kind == ObjectKind::Table
                        && Some(o.object.name.as_str()) == c.parent.as_deref()
                        && o.object.schema == c.object.schema
                })
        })
        .collect();
    if !orphans.is_empty() {
        let key = format!("{prefix}/f:Other");
        let open = tree.open(&key);
        let mut row = Row::new(
            rid(&key),
            depth,
            RowIcon::Lucide("folder"),
            "Other".into(),
            RowAction::Toggle(sid, key.clone()),
        );
        row.meta = Some(orphans.len().to_string());
        row.expander = Some(open);
        rows.push(row);
        if open {
            for c in orphans {
                let mut row = Row::new(
                    rid(&format!("{key}/c:{}", c.object.name)),
                    depth + 1,
                    RowIcon::Lucide(kind_icon(c.kind)),
                    c.object.name.clone(),
                    RowAction::None,
                );
                row.meta = c.parent.clone();
                rows.push(row);
            }
        }
    }
}

/// The rows of one open connection: itself, then (unfolded) its databases and objects.
fn session_rows(l: &Live, rows: &mut Vec<Row>, base: usize, tint: Hsla, filter: &str) {
    let sid = l.session.session_id;
    let tree = l.tree;
    let rid = |key: &str| format!("{sid}:{key}");
    let conn_open = tree.open("conn");
    let mut row = Row::new(
        rid("conn"),
        base,
        RowIcon::Db(l.session.driver.clone()),
        l.session.name.clone(),
        RowAction::Focus(sid),
    );
    row.tint = Some(tint);
    row.expander = Some(conn_open);
    row.strong = l.selected;
    row.selected = l.selected;
    row.live = true;
    row.chevron = Some(RowAction::Toggle(sid, "conn".into()));
    row.menu = Some(ConnMenu::Open(sid, None));
    rows.push(row);
    if !conn_open {
        return;
    }
    // A broker has no databases or tables: one row opens its console.
    if l.session.driver == "mqtt" {
        rows.push(Row::new(
            rid("console"),
            base + 1,
            RowIcon::Lucide("radio-tower"),
            "Console".into(),
            RowAction::OpenConsole(sid),
        ));
        return;
    }
    let redis = l.session.driver == "redis";
    // Drivers without databases (none today) fall back to the current objects.
    let databases: Vec<String> = if tree.databases.is_empty() {
        vec![tree.current.clone().unwrap_or_default()]
    } else {
        tree.databases.clone()
    };
    for db in databases {
        let key = format!("db:{db}");
        let current = tree.current.as_deref() == Some(db.as_str());
        let loaded = current || tree.other.contains_key(&db);
        let open = tree.open(&key) && loaded;
        let loading = tree.loading.contains(&db);
        // Redis databases open their key browser; their keys are not listed here.
        let action = if redis {
            RowAction::OpenKeys(sid, db.clone())
        } else if loaded {
            RowAction::Toggle(sid, key.clone())
        } else {
            RowAction::LoadDatabase(sid, db.clone())
        };
        let mut row = Row::new(
            rid(&key),
            base + 1,
            RowIcon::Lucide("database"),
            db.clone(),
            action,
        );
        // Databases with a session (the connection's own, or one opened for a table
        // there) are connected: coloured, with a dot.
        let connected = current || tree.db_sessions.contains_key(&db);
        row.tint = connected.then_some(tint);
        row.live = connected;
        row.menu = Some(ConnMenu::Database(DatabaseMenu {
            ws: sid,
            database: db.clone(),
            current,
            open: tree.db_sessions.contains_key(&db),
            driver: l.session.driver.clone(),
        }));
        row.meta = if loading {
            Some("Loading…".into())
        } else if tree.opening.contains(&db) {
            Some("Opening…".into())
        } else {
            tree.errors.get(&db).map(|_| "error".into())
        };
        row.expander = (!redis).then_some(open);
        rows.push(row);
        if !open || redis {
            continue;
        }
        let objects: Vec<&SchemaObject> = if current {
            l.objects.iter().collect()
        } else {
            tree.other
                .get(&db)
                .map(|v| v.iter().collect())
                .unwrap_or_default()
        };
        // Group by schema when the driver reports schemas (PostgreSQL).
        let mut schemas: Vec<String> = objects
            .iter()
            .filter_map(|o| o.object.schema.clone())
            .collect();
        schemas.sort();
        schemas.dedup();
        if schemas.is_empty() {
            object_rows(l, rows, &key, base + 2, Some(&db), &objects, filter);
        } else {
            for schema in schemas {
                let skey = format!("{key}/s:{schema}");
                let sopen = tree.open(&skey) || !filter.is_empty();
                let in_schema: Vec<&SchemaObject> = objects
                    .iter()
                    .copied()
                    .filter(|o| o.object.schema.as_deref() == Some(schema.as_str()))
                    .collect();
                let mut row = Row::new(
                    rid(&skey),
                    base + 2,
                    RowIcon::Lucide("layers"),
                    schema.clone(),
                    RowAction::Toggle(sid, skey.clone()),
                );
                row.meta = Some("schema".into());
                row.expander = Some(sopen);
                rows.push(row);
                if sopen {
                    object_rows(l, rows, &skey, base + 3, Some(&db), &in_schema, filter);
                }
            }
        }
    }
}

impl ThaanApp {
    /// Loads the database list of the connection in front after connecting or
    /// refreshing; opens the path to its current database.
    pub(super) fn load_databases(&mut self, sid: SessionId, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.list_databases(sid) })
                .await;
            this.update(cx, |this, cx| {
                // The session may have been closed meanwhile; then there is no tree for it.
                let Some(t) = this.tree_of(sid) else { return };
                t.other.clear();
                t.errors.clear();
                match res {
                    Ok(list) => {
                        t.current = list.current.clone();
                        t.databases = list.databases;
                        t.expanded.insert("conn".into());
                        if let Some(db) = list.current {
                            let key = format!("db:{db}");
                            t.expanded.insert(key.clone());
                            t.expanded
                                .insert(format!("{key}/f:{:?}", ObjectKind::Table));
                            // PostgreSQL: open the `public` schema too.
                            t.expanded.insert(format!("{key}/s:public"));
                            t.expanded
                                .insert(format!("{key}/s:public/f:{:?}", ObjectKind::Table));
                        }
                    }
                    Err(e) => {
                        if this.active_sid() == Some(sid) {
                            this.set_status(format!("Databases: {e}"), true, cx)
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn load_database(&mut self, sid: SessionId, db: String, cx: &mut Context<Self>) {
        let Some(t) = self.tree_of(sid) else { return };
        t.loading.insert(db.clone());
        t.expanded.insert(format!("db:{db}"));
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let name = db.clone();
            let res = cx
                .background_spawn(async move { engine.list_objects_in(sid, &name) })
                .await;
            this.update(cx, |this, cx| {
                let Some(t) = this.tree_of(sid) else { return };
                t.loading.remove(&db);
                match res {
                    Ok(objects) => {
                        t.other.insert(db.clone(), objects);
                        t.errors.remove(&db);
                    }
                    Err(e) => {
                        t.errors.insert(db.clone(), e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens an object of another database of the connection in front, as a new tab.
    /// The connection's own session and tabs stay as they are: the first table of that
    /// database opens a second session on it in the background, and later ones reuse it.
    fn open_in_database(
        &mut self,
        database: String,
        object: ObjectRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_database(database, DbThen::OpenTable(object), window, cx)
    }

    /// Runs `then` on a session for `database` of the connection in front: its own
    /// session when that is its database, else a second session opened (once, in the
    /// background) for it.
    fn with_database(
        &mut self,
        database: String,
        then: DbThen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_sid() else { return };
        if self.tree.current.as_deref() == Some(database.as_str()) {
            return self.db_then(None, then, window, cx);
        }
        if let Some(&sid) = self.tree.db_sessions.get(&database) {
            return self.db_then(Some((sid, database)), then, window, cx);
        }
        if !self.tree.opening.insert(database.clone()) {
            return;
        }
        self.set_status(format!("Opening {database}…"), false, cx);
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let db = database.clone();
            let res = cx
                .background_spawn(async move { engine.open_database(ws, &db) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(t) = this.tree_of(ws) else {
                    // The connection closed meanwhile: close the late session too.
                    if let Ok(info) = &res {
                        this.engine.close(info.session_id).ok();
                    }
                    return;
                };
                t.opening.remove(&database);
                match res {
                    Ok(info) => {
                        t.db_sessions.insert(database.clone(), info.session_id);
                        if this.active_sid() == Some(ws) {
                            this.status = None;
                            this.db_then(Some((info.session_id, database)), then, window, cx);
                        }
                    }
                    Err(e) => {
                        if this.active_sid() == Some(ws) {
                            this.set_status(format!("Could not open {database}: {e}"), true, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn db_then(
        &mut self,
        on: Option<(SessionId, String)>,
        then: DbThen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match then {
            DbThen::OpenTable(object) => self.open_table_on(on, object, window, cx),
            DbThen::OpenKeys => self.open_keys(on, window, cx),
            DbThen::NewQuery | DbThen::NewQueryWith(_) => {
                let text = match &then {
                    DbThen::NewQueryWith(t) => Some(t.clone()),
                    _ => None,
                };
                self.new_query_tab(None, window, cx);
                if let (Some(text), Some(Tab::Query(q))) = (text, self.tabs.get(self.active)) {
                    q.editor.update(cx, |e, cx| e.set_value(text, window, cx));
                }
                if let (Some((sid, db)), Some(Tab::Query(q))) = (on, self.tabs.get_mut(self.active))
                {
                    q.sid = sid;
                    q.database = Some(db);
                }
                cx.notify();
            }
        }
    }

    /// Closes the second session opened for `database` and its tabs; the database stays
    /// in the tree, muted, until something of it is opened again.
    fn close_database(&mut self, ws: SessionId, database: &str, cx: &mut Context<Self>) {
        let Some(sid) = self
            .tree_of(ws)
            .and_then(|t| t.db_sessions.remove(database))
        else {
            return;
        };
        let shown = self.tabs.get(self.active).map(tab_session);
        let before = self
            .tabs
            .iter()
            .take(self.active)
            .filter(|t| tab_session(t) != sid)
            .count();
        self.tabs.retain(|t| tab_session(t) != sid);
        self.active = before.min(self.tabs.len().saturating_sub(1));
        if shown == Some(sid) && self.tabs.is_empty() {
            self.on_home = true;
        }
        let engine = self.engine.clone();
        cx.background_spawn(async move { engine.close(sid).ok() })
            .detach();
        self.set_status(format!("Closed {database}"), false, cx);
    }

    /// The open sessions, the one in front first.
    fn lives(&self) -> Vec<Live<'_>> {
        let mut out = Vec::new();
        if let Some(s) = &self.session {
            out.push(Live {
                session: s,
                tree: &self.tree,
                objects: &self.objects,
                selected: true,
            });
        }
        for p in &self.parked {
            out.push(Live {
                session: &p.session,
                tree: &p.tree,
                objects: &p.objects,
                selected: false,
            });
        }
        out
    }

    fn tree_rows(&self, filter: &str, accent: Hsla) -> Vec<Row> {
        let mut rows = Vec::new();
        let lives = self.lives();
        let tint = |s: &SessionInfo| s.color.as_deref().and_then(parse_hex).unwrap_or(accent);
        let mut shown: HashSet<SessionId> = HashSet::new();
        // A saved connection: its open session's rows, or one muted row that connects.
        let mut conn = |rows: &mut Vec<Row>, c: &ConnectionConfig, base: usize| {
            if let Some(l) = lives.iter().find(|l| l.session.connection_id == c.id) {
                shown.insert(l.session.session_id);
                let first = rows.len();
                session_rows(l, rows, base, tint(l.session), filter);
                if let Some(row) = rows.get_mut(first) {
                    row.menu = Some(ConnMenu::Open(
                        l.session.session_id,
                        Some(Box::new(c.clone())),
                    ));
                }
                return;
            }
            if !filter.is_empty() && !c.name.to_lowercase().contains(filter) {
                return;
            }
            let mut row = Row::new(
                format!("saved:{}", c.id),
                base,
                RowIcon::Db(c.driver.clone()),
                c.name.clone(),
                RowAction::Connect(Box::new(c.clone())),
            );
            row.expander = Some(false);
            if self.connecting_id.as_deref() == Some(c.id.as_str()) {
                row.meta = Some("Connecting…".into());
            }
            row.menu = Some(ConnMenu::Closed(Box::new(c.clone())));
            rows.push(row);
        };
        for g in &self.home.folders {
            let members: Vec<&ConnectionConfig> = self
                .connections
                .iter()
                .filter(|c| c.folder.as_deref() == Some(g.as_str()))
                .collect();
            let open = !filter.is_empty() || !self.home.collapsed.contains(g);
            let mut row = Row::new(
                format!("group:{g}"),
                0,
                RowIcon::Lucide(if open { "folder-open" } else { "folder" }),
                g.clone(),
                RowAction::Group(g.clone()),
            );
            row.meta = Some(members.len().to_string());
            row.expander = Some(open);
            rows.push(row);
            if open {
                for c in members {
                    conn(&mut rows, c, 1);
                }
            }
        }
        for c in &self.connections {
            let grouped = c
                .folder
                .as_ref()
                .is_some_and(|f| self.home.folders.contains(f));
            if !grouped {
                conn(&mut rows, c, 0);
            }
        }
        // Open sessions without a saved connection (quick connect, a switched database
        // of an unsaved one) go last.
        for l in &lives {
            if !shown.contains(&l.session.session_id) {
                session_rows(l, &mut rows, 0, tint(l.session), filter);
            }
        }
        rows
    }

    fn run_row_action(&mut self, action: RowAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            RowAction::None => {}
            RowAction::Toggle(sid, key) => {
                if let Some(t) = self.tree_of(sid) {
                    t.toggle(&key);
                }
                cx.notify();
            }
            RowAction::LoadDatabase(sid, db) => self.load_database(sid, db, cx),
            RowAction::Open {
                sid,
                database,
                object,
            } => {
                self.activate(sid, cx);
                // From the home screen's tree too: opening a table shows the workspace.
                self.on_home = false;
                match database {
                    None => self.open_table(object, window, cx),
                    Some(db) => self.open_in_database(db, object, window, cx),
                }
            }
            RowAction::OpenKeys(sid, db) => {
                self.activate(sid, cx);
                self.with_database(db, DbThen::OpenKeys, window, cx);
            }
            RowAction::OpenConsole(sid) => {
                self.activate(sid, cx);
                self.open_console(window, cx);
            }
            RowAction::Connect(c) => self.connect(*c, window, cx),
            RowAction::Focus(sid) => {
                // Unfolds the connection; the main area and the tab on screen stay.
                // Only the chevron folds, so a double-click never undoes the first click.
                if let Some(t) = self.tree_of(sid) {
                    t.expanded.insert("conn".into());
                }
                cx.notify();
            }
            RowAction::Group(g) => {
                if !self.home.collapsed.remove(&g) {
                    self.home.collapsed.insert(g);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_object_tree(&self, filter: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let rows = self.tree_rows(filter, theme.primary);
        let this = cx.entity();
        let mut list = v_flex().id("tree").flex_1().px_2().pb_2().gap_0p5();
        for row in rows {
            list = list.child(self.render_tree_row(row, &this, &theme, cx));
        }
        // A visible scrollbar: a server can list hundreds of tables.
        list.overflow_y_scrollbar().into_any_element()
    }

    /// The rows under one open connection (databases, folders, objects), for the home
    /// screen, where the connection row itself is drawn by home. Empty when the
    /// connection is not open or is folded.
    pub(super) fn home_tree_rows(
        &self,
        connection_id: &str,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let lives = self.lives();
        let Some(l) = lives
            .iter()
            .find(|l| l.session.connection_id == connection_id)
        else {
            return Vec::new();
        };
        let tint = l
            .session
            .color
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(theme.primary);
        let mut rows = Vec::new();
        session_rows(l, &mut rows, depth, tint, "");
        drop(lives);
        let this = cx.entity();
        rows.into_iter()
            .skip(1)
            .map(|row| self.render_tree_row(row, &this, &theme, cx))
            .collect()
    }

    /// For an open connection: its session and whether its tree is unfolded (its home row
    /// shows ⌄ instead of ›).
    pub(super) fn home_tree_open(&self, connection_id: &str) -> Option<(SessionId, bool)> {
        self.lives()
            .iter()
            .find(|l| l.session.connection_id == connection_id)
            .map(|l| (l.session.session_id, l.tree.open("conn")))
    }

    /// Folds or unfolds an open connection's tree from its home row.
    pub(super) fn toggle_home_tree(&mut self, sid: SessionId, cx: &mut Context<Self>) {
        if let Some(t) = self.tree_of(sid) {
            t.toggle("conn");
        }
        cx.notify();
    }

    fn render_tree_row(
        &self,
        row: Row,
        this: &Entity<ThaanApp>,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = row.action.clone();
        let chevron_action = row.chevron.clone().unwrap_or_else(|| row.action.clone());
        let hover = theme.sidebar_accent;
        let chevron = match row.expander {
            Some(true) => Some("chevron-down"),
            Some(false) => Some("chevron-right"),
            None => None,
        };
        let clickable = !matches!(action, RowAction::None);
        let icon_color = row.tint.unwrap_or(theme.muted_foreground);
        let row_icon = match &row.icon {
            RowIcon::Lucide(name) => icon(name),
            RowIcon::Db(driver) => db_icon(driver),
        };
        let menu = row.menu.clone();
        let menu_app = this.clone();
        let folders = self.home.folders.clone();
        let menu_row_id = row.id.clone();
        let el = h_flex()
            .id(SharedString::from(format!("tree-{}", row.id)))
            .h(px(ROW_H))
            .flex_none()
            .gap_1p5()
            .pr_2()
            .pl(px(4. + row.depth as f32 * 14.))
            .rounded(px(6.))
            .text_sm()
            // The row a right-click menu is open for keeps the hover background, so it
            // is clear what the menu acts on.
            .when(
                row.selected || self.menu_row.as_deref() == Some(row.id.as_str()),
                |d| d.bg(theme.sidebar_accent),
            )
            .when(clickable, |d| {
                d.cursor_pointer().hover(move |s| s.bg(hover))
            })
            // The chevron folds; the rest of the row runs its action.
            .child(
                div()
                    .id(SharedString::from(format!("tree-x-{}", row.id)))
                    .w(px(14.))
                    .flex_none()
                    .when_some(chevron, |d, c| {
                        d.child(icon(c).size_3p5().text_color(theme.muted_foreground))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.run_row_action(chevron_action.clone(), window, cx);
                            }))
                    }),
            )
            .child(row_icon.size_4().flex_none().text_color(icon_color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(row.strong, |d| d.font_medium())
                    .child(row.label),
            )
            .when(row.live, |d| {
                d.child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(theme.success),
                )
            })
            .children(row.meta.map(|m| {
                div()
                    .flex_none()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(m)
            }))
            .when(clickable, |d| {
                d.on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    // A closed connection connects on a double-click (its chevron
                    // connects on one click); a single click on its row does nothing.
                    if matches!(action, RowAction::Connect(_)) && ev.click_count() < 2 {
                        return;
                    }
                    this.run_row_action(action.clone(), window, cx)
                }))
            });
        match menu {
            Some(m) => el
                .context_menu(move |menu, window, cx| {
                    // Runs once each time the menu opens: mark the row, and unmark it
                    // when the menu closes (picked or dismissed).
                    let id = SharedString::from(menu_row_id.clone());
                    menu_app.update(cx, |app, cx| {
                        app.menu_row = Some(id.clone());
                        cx.notify();
                    });
                    let app = menu_app.clone();
                    cx.subscribe_self(move |_, _: &DismissEvent, cx| {
                        app.update(cx, |app, cx| {
                            if app.menu_row.as_ref() == Some(&id) {
                                app.menu_row = None;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                    conn_tree_menu(menu, &menu_app, &m, &folders, window, cx)
                })
                .into_any_element(),
            None => el.into_any_element(),
        }
    }
}

/// The right-click menu of a connection row in the tree.
fn conn_tree_menu(
    menu: PopupMenu,
    this: &Entity<ThaanApp>,
    m: &ConnMenu,
    folders: &[String],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    match m.clone() {
        // The same menu as the saved connections on Home: Connect, Edit, Duplicate,
        // Copy, Move to group, New group, Delete.
        ConnMenu::Closed(c) => super::home::conn_menu(menu, this, &c, false, folders, window, cx),
        ConnMenu::Object(o) => object_menu(menu, this, &o, window, cx),
        ConnMenu::Database(d) => database_menu(menu, this, &d),
        ConnMenu::Open(sid, saved) => {
            let (a, b, d) = (this.clone(), this.clone(), this.clone());
            let menu = menu
                .item(
                    PopupMenuItem::new("New query")
                        .icon(icon("file-code"))
                        .on_click(move |_, window, cx| {
                            a.update(cx, |app, cx| {
                                app.activate(sid, cx);
                                app.new_query_tab(None, window, cx);
                            })
                        }),
                )
                .item(
                    PopupMenuItem::new("Refresh")
                        .icon(icon("refresh-cw"))
                        .on_click(move |_, _, cx| {
                            b.update(cx, |app, cx| {
                                // Reloads its tree without changing the tab on screen.
                                app.load_databases(sid, cx);
                                if app.active_sid() == Some(sid) {
                                    app.refresh_schema(cx);
                                }
                            })
                        }),
                );
            let menu = match &saved {
                Some(c) => super::home::copy_submenu(menu, this, c, window, cx),
                None => menu,
            };
            menu.separator().item(
                PopupMenuItem::new("Disconnect")
                    .icon(icon("unplug"))
                    .on_click(move |_, _, cx| d.update(cx, |app, cx| app.close_session(sid, cx))),
            )
        }
    }
}

/// The right-click menu of a table, view or collection: open its data or structure, a
/// ready query, copies of its name and SQL, and a refresh of its database.
fn object_menu(
    menu: PopupMenu,
    this: &Entity<ThaanApp>,
    o: &ObjectMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mongo = o.driver == "mongodb";
    // Tables of another database open on their own session; the query and CREATE text
    // need the connection's own session, so they are offered for its database only.
    let own_db = o.database.is_none();
    let open = |structure: bool| {
        let (app, o) = (this.clone(), o.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            app.update(cx, |app, cx| app.open_object(&o, structure, window, cx))
        }
    };
    let (q_app, q_o) = (this.clone(), o.clone());
    let (r_app, r_sid) = (this.clone(), o.sid);
    let menu = menu
        .item(
            PopupMenuItem::new("Open data")
                .icon(icon("table"))
                .on_click(open(false)),
        )
        .item(
            PopupMenuItem::new("Open structure")
                .icon(icon("list"))
                .disabled(mongo)
                .on_click(open(true)),
        )
        .item(
            PopupMenuItem::new("New query")
                .icon(icon("file-code"))
                .disabled(!own_db)
                .on_click(move |_, window, cx| {
                    q_app.update(cx, |app, cx| {
                        app.activate(q_o.sid, cx);
                        let text = super::command_bar::starter_command(&q_o.driver, &q_o.object);
                        app.new_query_tab(None, window, cx);
                        if let Some(Tab::Query(q)) = app.tabs.get(app.active) {
                            q.editor.update(cx, |e, cx| e.set_value(text, window, cx));
                        }
                    })
                }),
        )
        .separator();
    let o2 = o.clone();
    let copy_app = this.clone();
    let menu = menu.submenu_with_icon(
        Some(icon("clipboard")),
        "Copy",
        window,
        cx,
        move |m, _, _| {
            let name = o2.object.name.clone();
            let quoted =
                thaan_core::dialect::Dialect::for_driver(&o2.driver).quote_object(&o2.object);
            let select = super::command_bar::starter_command(&o2.driver, &o2.object);
            let (d_app, d_o) = (copy_app.clone(), o2.clone());
            let text = |t: String| {
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    cx.write_to_clipboard(ClipboardItem::new_string(t.clone()))
                }
            };
            m.item(PopupMenuItem::new("Name").on_click(text(name)))
                .item(
                    PopupMenuItem::new(if o2.driver == "mongodb" {
                        "Collection expression"
                    } else {
                        "Qualified name"
                    })
                    .on_click(text(if o2.driver == "mongodb" {
                        let n = thaan_core::doc::Doc::String(o2.object.name.clone()).to_pretty();
                        format!("db.getCollection({n})")
                    } else {
                        quoted
                    })),
                )
                .item(
                    PopupMenuItem::new(if o2.driver == "mongodb" {
                        "find() command"
                    } else {
                        "SELECT statement"
                    })
                    .on_click(text(select)),
                )
                .item(
                    PopupMenuItem::new("CREATE statement")
                        .disabled(o2.driver == "mongodb" || o2.database.is_some())
                        .on_click(move |_, _, cx| {
                            d_app
                                .update(cx, |app, cx| app.copy_ddl(d_o.sid, d_o.object.clone(), cx))
                        }),
                )
        },
    );
    let menu = menu.separator().item(
        PopupMenuItem::new("Refresh")
            .icon(icon("refresh-cw"))
            .on_click(move |_, _, cx| {
                r_app.update(cx, |app, cx| {
                    app.load_databases(r_sid, cx);
                    if app.active_sid() == Some(r_sid) {
                        app.refresh_schema(cx);
                    }
                })
            }),
    );
    // Export opens the object's tab and exports what it shows (filters included).
    let (e_app, e_o) = (this.clone(), o.clone());
    let menu = menu.submenu_with_icon(
        Some(icon("download")),
        "Export",
        window,
        cx,
        move |mut m, _, _| {
            for (label, format) in [
                ("CSV", ExportFormat::Csv),
                ("JSON", ExportFormat::Json),
                ("SQL INSERT", ExportFormat::SqlInsert),
            ] {
                let (app, o) = (e_app.clone(), e_o.clone());
                m = m.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.open_object(&o, false, window, cx);
                        if matches!(app.tabs.get(app.active), Some(Tab::Table(t)) if t.object == o.object)
                        {
                            app.export_table_to_file(format, cx);
                        }
                    })
                }));
            }
            m
        },
    );
    // Changing the object itself: each opens a dialog that shows the statements first.
    use super::table_ops::OpKind;
    let table = o.kind == ObjectKind::Table;
    let op = |kind: OpKind| {
        let (app, o) = (this.clone(), o.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            app.update(cx, |app, cx| {
                app.open_op_dialog(o.target(), kind, window, cx)
            })
        }
    };
    let red = cx.theme().danger;
    let danger =
        |label: &'static str, ic: &'static str| super::home::danger_item_with(label, ic, red);
    menu.separator()
        .item(
            PopupMenuItem::new("Rename")
                .icon(icon("pencil"))
                .on_click(op(OpKind::Rename)),
        )
        .item(
            PopupMenuItem::new("Duplicate")
                .icon(icon("copy"))
                .disabled(!table)
                .on_click(op(OpKind::Duplicate)),
        )
        .separator()
        .when(table, |m| {
            m.item(danger("Truncate", "eraser").on_click(op(OpKind::Truncate)))
        })
        .item(danger("Drop", "trash").on_click(op(OpKind::Drop)))
}

impl ThaanApp {
    /// Opens an object from its menu, on its data or its structure.
    fn open_object(
        &mut self,
        o: &ObjectMenu,
        structure: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_row_action(
            RowAction::Open {
                sid: o.sid,
                database: o.database.clone(),
                object: o.object.clone(),
            },
            window,
            cx,
        );
        if structure {
            self.show_structure(&o.object, cx);
        }
    }

    /// Switches the table tab of `object` (if it is on screen) to Structure and loads its
    /// CREATE text off the UI thread. A tab still opening on another database's session
    /// just shows its data.
    pub(super) fn show_structure(&mut self, object: &ObjectRef, cx: &mut Context<Self>) {
        let ix = self.active;
        let Some(Tab::Table(t)) = self.tabs.get_mut(ix) else {
            return;
        };
        if &t.object != object {
            return;
        }
        t.view = TableView::Structure;
        cx.notify();
        if t.ddl.is_some() {
            return;
        }
        let (ws, sid, object) = (t.ws, t.sid, t.object.clone());
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.ddl(sid, &object) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(Tab::Table(t)) = this.tab_at(ws, ix) {
                    t.ddl = res.ok();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Copies an object's CREATE statement; the text comes from the server, off the UI
    /// thread.
    fn copy_ddl(&mut self, sid: SessionId, object: ObjectRef, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        self.set_status(
            format!("Copying the CREATE statement of {}…", object.name),
            false,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let name = object.name.clone();
            let res = cx
                .background_spawn(async move { engine.ddl(sid, &object) })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    this.set_status(format!("Copied the CREATE statement of {name}"), false, cx);
                }
                Err(e) => this.set_status(format!("CREATE statement: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }
}

/// The right-click menu of a database row: a query on it, refresh, copy its name, and
/// close the second session opened for it.
fn database_menu(menu: PopupMenu, this: &Entity<ThaanApp>, d: &DatabaseMenu) -> PopupMenu {
    let (q_app, q_d) = (this.clone(), d.clone());
    let (r_app, r_d) = (this.clone(), d.clone());
    let (c_app, c_d) = (this.clone(), d.clone());
    let name = d.database.clone();
    let (n_app, n_d) = (this.clone(), d.clone());
    let (t_app, t_d) = (this.clone(), d.clone());
    let mongo = d.driver == "mongodb";
    menu.item(
        PopupMenuItem::new("New query")
            .icon(icon("file-code"))
            .on_click(move |_, window, cx| {
                q_app.update(cx, |app, cx| {
                    app.activate(q_d.ws, cx);
                    app.with_database(q_d.database.clone(), DbThen::NewQuery, window, cx);
                })
            }),
    )
    // MongoDB: name a new collection. SQL: a query tab with a CREATE TABLE to edit
    // (a table needs its columns chosen), run on this database.
    .item(
        PopupMenuItem::new(if mongo { "Add collection" } else { "New table" })
            .icon(icon("table-2"))
            .on_click(move |_, window, cx| {
                n_app.update(cx, |app, cx| {
                    app.activate(n_d.ws, cx);
                    if n_d.driver == "mongodb" {
                        let target = super::table_ops::OpTarget {
                            ws: n_d.ws,
                            database: (!n_d.current).then(|| n_d.database.clone()),
                            object: ObjectRef {
                                schema: None,
                                name: String::new(),
                            },
                            kind: ObjectKind::Table,
                            driver: n_d.driver.clone(),
                        };
                        app.open_op_dialog(target, super::table_ops::OpKind::Create, window, cx);
                    } else {
                        let schema = (n_d.driver == "postgres").then(|| "public".to_string());
                        let sql = thaan_core::dialect::Dialect::for_driver(&n_d.driver)
                            .create_table_template(&ObjectRef {
                                schema,
                                name: "new_table".into(),
                            });
                        app.with_database(
                            n_d.database.clone(),
                            DbThen::NewQueryWith(sql),
                            window,
                            cx,
                        );
                    }
                })
            }),
    )
    .item(
        PopupMenuItem::new("Transfer data")
            .icon(icon("arrow-left-right"))
            .on_click(move |_, window, cx| {
                t_app.update(cx, |app, cx| {
                    let conn = app
                        .session_info(t_d.ws)
                        .map(|s| s.connection_id.clone())
                        .unwrap_or_default();
                    app.open_transfer_dialog(Some((conn, Some(t_d.database.clone()))), window, cx);
                })
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Refresh")
            .icon(icon("refresh-cw"))
            .on_click(move |_, _, cx| {
                r_app.update(cx, |app, cx| {
                    if r_d.current {
                        app.load_databases(r_d.ws, cx);
                        if app.active_sid() == Some(r_d.ws) {
                            app.refresh_schema(cx);
                        }
                    } else {
                        app.load_database(r_d.ws, r_d.database.clone(), cx);
                    }
                })
            }),
    )
    .item(
        PopupMenuItem::new("Copy name")
            .icon(icon("clipboard"))
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Close database connection")
            .icon(icon("unplug"))
            .disabled(d.current || !d.open)
            .on_click(move |_, _, cx| {
                c_app.update(cx, |app, cx| app.close_database(c_d.ws, &c_d.database, cx))
            }),
    )
}
