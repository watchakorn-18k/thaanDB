//! The demo window: a top bar, a sidebar (the sample database's tables, a SQL query, the
//! MQTT console) and the picked pane. Panes use the desktop app's grid and theme.

use crate::grid::{track_focus, ResultGrid, GRID_HEADER_H};
use crate::mqtt_ws::{Client, Message};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;
use std::time::Duration;
use thaan_core::engine::{Engine, QueryRequest, RunScope, SessionId};
use thaan_core::schema::ObjectRef;
use thaan_core::table::PageRequest;
use thaan_core::ResultSet;

const RELEASES: &str = "https://github.com/watchakorn-18k/thaanDB/releases/latest";
const PAGE: u64 = 50;
/// Messages kept in the MQTT list.
const KEEP: usize = 500;

fn icon(name: &str) -> Icon {
    Icon::empty().path(SharedString::from(format!("icons/{name}.svg")))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Table(usize),
    Query,
    Mqtt,
}

pub struct Demo {
    engine: Option<Arc<Engine>>,
    sid: Option<SessionId>,
    startup_error: Option<String>,
    pane: Pane,
    grid: Entity<TableState<ResultGrid>>,
    page: u64,
    total: u64,
    status: Option<(String, bool)>,
    editor: Entity<EditorState>,
    query_grid: Entity<TableState<ResultGrid>>,
    query_status: Option<(String, bool)>,
    // MQTT console.
    broker: Entity<InputState>,
    sub_filter: Entity<InputState>,
    pub_topic: Entity<InputState>,
    pub_payload: Entity<InputState>,
    client: Option<Client>,
    messages: Vec<Message>,
    mqtt_note: Option<(String, bool)>,
}

impl Demo {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grid = cx.new(|cx| TableState::new(ResultGrid::new(ResultSet::default()), window, cx));
        track_focus(&grid, cx);
        let query_grid =
            cx.new(|cx| TableState::new(ResultGrid::new(ResultSet::default()), window, cx));
        track_focus(&query_grid, cx);
        let editor = cx.new(|cx| EditorState::new(window, cx).line_number(true));
        editor.update(cx, |e, cx| e.set_value(crate::demo::STARTER, window, cx));
        let input = |value: &str, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let s = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let v = value.to_string();
            s.update(cx, |s, cx| s.set_value(v, window, cx));
            s
        };
        let broker = input("wss://test.mosquitto.org:8081", "wss://broker:port", window, cx);
        let sub_filter = input("thaan/demo/#", "Topic filter", window, cx);
        let pub_topic = input("thaan/demo/hello", "Topic", window, cx);
        let pub_payload = input("{\"from\":\"ThaanDB web\"}", "Payload", window, cx);
        cx.subscribe_in(&pub_payload, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.publish(cx);
            }
        })
        .detach();

        let (engine, sid, startup_error) = match crate::demo::open() {
            Ok((e, info)) => (Some(e), Some(info.session_id), None),
            Err(e) => (None, None, Some(e.to_string())),
        };
        let mut this = Self {
            engine,
            sid,
            startup_error,
            pane: Pane::Table(2),
            grid,
            page: 0,
            total: 0,
            status: None,
            editor,
            query_grid,
            query_status: None,
            broker,
            sub_filter,
            pub_topic,
            pub_payload,
            client: None,
            messages: Vec::new(),
            mqtt_note: None,
        };
        this.load_table(cx);
        this
    }

    // ---- tables ---------------------------------------------------------------------

    fn open_table(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.pane = Pane::Table(ix);
        self.page = 0;
        self.load_table(cx);
    }

    /// The sample data lives in memory, so a page comes back in a millisecond or two:
    /// it is read right here (the browser build runs on one thread anyway).
    fn load_table(&mut self, cx: &mut Context<Self>) {
        let (Some(engine), Some(sid), Pane::Table(ix)) = (&self.engine, self.sid, self.pane) else {
            return;
        };
        let table = ObjectRef::new(crate::demo::TABLES[ix]);
        let info = engine.describe_table(sid, &table).ok();
        let req = PageRequest {
            table,
            filters: vec![],
            raw_where: None,
            projection: None,
            raw_sort: None,
            sort: vec![],
            page: self.page,
            page_size: PAGE,
        };
        match engine.fetch_page(sid, &req) {
            Ok(p) => {
                self.total = p.total_rows;
                self.status = Some((
                    format!("{} rows · {} ms", p.total_rows, p.result.elapsed_ms),
                    false,
                ));
                let offset = p.page * p.page_size;
                self.grid.update(cx, |g, cx| {
                    let d = g.delegate_mut();
                    d.set(p.result);
                    if let Some(info) = &info {
                        d.set_table_info(info);
                    }
                    d.set_row_offset(offset);
                    g.refresh(cx);
                });
            }
            Err(e) => self.status = Some((e.to_string(), true)),
        }
        cx.notify();
    }

    // ---- query ----------------------------------------------------------------------

    fn run_query(&mut self, cx: &mut Context<Self>) {
        let (Some(engine), Some(sid)) = (&self.engine, self.sid) else {
            return;
        };
        let sql = self.editor.read(cx).value().to_string();
        match engine.run_query(&QueryRequest {
            session_id: sid,
            sql,
            scope: RunScope::All,
            params: Default::default(),
        }) {
            Ok(results) => {
                let failed = results.iter().find_map(|r| r.error.clone());
                // Show the last statement that returned rows (or the last one).
                let shown = results
                    .iter()
                    .rev()
                    .find_map(|r| r.result.clone().filter(|rs| !rs.columns.is_empty()))
                    .or_else(|| results.last().and_then(|r| r.result.clone()));
                self.query_status = Some(match (&failed, &shown) {
                    (Some(e), _) => (e.clone(), true),
                    (None, Some(rs)) if !rs.columns.is_empty() => (
                        format!(
                            "{} rows · {} ms · {} statement{}",
                            rs.rows.len(),
                            rs.elapsed_ms,
                            results.len(),
                            if results.len() == 1 { "" } else { "s" }
                        ),
                        false,
                    ),
                    (None, Some(rs)) => (
                        format!("{} rows affected", rs.rows_affected.unwrap_or(0)),
                        false,
                    ),
                    (None, None) => ("Nothing to run".into(), false),
                });
                if let Some(rs) = shown.filter(|rs| !rs.columns.is_empty()) {
                    self.query_grid.update(cx, |g, cx| {
                        g.delegate_mut().set(rs);
                        g.refresh(cx);
                    });
                }
            }
            Err(e) => self.query_status = Some((e.to_string(), true)),
        }
        cx.notify();
    }

    // ---- MQTT -----------------------------------------------------------------------

    fn toggle_broker(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.client.take() {
            c.close();
            self.mqtt_note = Some(("Disconnected".into(), false));
            cx.notify();
            return;
        }
        let url = self.broker.read(cx).value().trim().to_string();
        match Client::connect(&url) {
            Ok(c) => {
                self.client = Some(c);
                self.mqtt_note = Some((format!("Connecting to {url}…"), false));
                self.poll(cx);
            }
            Err(e) => self.mqtt_note = Some((e, true)),
        }
        cx.notify();
    }

    /// Drains the socket's inbox every 100 ms while connected, and pings every 30 s.
    fn poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            let mut subscribed = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                ticks += 1;
                let go_on = this.update(cx, |this, cx| {
                    let Some(client) = this.client.as_ref() else {
                        return false;
                    };
                    if ticks % 300 == 0 {
                        client.ping();
                    }
                    let (connected, closed, error, new) = {
                        let mut inbox = client.inbox.borrow_mut();
                        (
                            inbox.connected,
                            inbox.closed,
                            inbox.error.take(),
                            std::mem::take(&mut inbox.messages),
                        )
                    };
                    if connected && !subscribed {
                        subscribed = true;
                        this.mqtt_note = Some(("Connected".into(), false));
                        this.subscribe(cx);
                    }
                    if let Some(e) = error {
                        this.mqtt_note = Some((e, true));
                    }
                    if !new.is_empty() {
                        this.messages.extend(new);
                        let extra = this.messages.len().saturating_sub(KEEP);
                        this.messages.drain(..extra);
                    }
                    cx.notify();
                    if closed {
                        this.client = None;
                    }
                    !closed
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn subscribe(&mut self, cx: &mut Context<Self>) {
        let filter = self.sub_filter.read(cx).value().trim().to_string();
        let Some(c) = self.client.as_mut() else {
            self.mqtt_note = Some(("Connect to a broker first".into(), true));
            return;
        };
        self.mqtt_note = Some(match c.subscribe(&filter, 0) {
            Ok(()) => (format!("Subscribed to {filter}"), false),
            Err(e) => (e, true),
        });
        cx.notify();
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let topic = self.pub_topic.read(cx).value().trim().to_string();
        let payload = self.pub_payload.read(cx).value().to_string();
        let Some(c) = self.client.as_mut() else {
            self.mqtt_note = Some(("Connect to a broker first".into(), true));
            cx.notify();
            return;
        };
        self.mqtt_note = Some(match c.publish(&topic, payload.as_bytes(), 0, false) {
            Ok(()) => (format!("Published {} bytes to {topic}", payload.len()), false),
            Err(e) => (e, true),
        });
        cx.notify();
    }

    // ---- render ---------------------------------------------------------------------

    fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .h(px(44.))
            .flex_none()
            .px_4()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("ThaanDB"))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Browser demo · a sample shop database in your browser"),
            )
            .child(div().flex_1())
            .child(
                Button::new("get-app")
                    .icon(icon("download"))
                    .label("Get the desktop app")
                    .primary()
                    .small()
                    .on_click(|_, _, cx| cx.open_url(RELEASES)),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row = |id: SharedString, ic: &'static str, label: String, on: bool| {
            h_flex()
                .id(id)
                .h(px(32.))
                .mx_2()
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .text_sm()
                .cursor_pointer()
                .when(on, |d| d.bg(theme.foreground.opacity(0.08)))
                .hover(|d| d.bg(theme.foreground.opacity(0.04)))
                .child(icon(ic).size_4().text_color(theme.muted_foreground))
                .child(label)
        };
        let heading = |t: &'static str| {
            div()
                .px_4()
                .pt_4()
                .pb_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let tables = crate::demo::TABLES.iter().enumerate().map(|(i, t)| {
            row(
                SharedString::from(format!("t-{t}")),
                "table",
                t.to_string(),
                self.pane == Pane::Table(i),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open_table(i, cx)))
        });
        v_flex()
            .w(px(240.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(heading("Demo shop · SQLite"))
            .children(tables)
            .child(
                row("query".into(), "file-code", "SQL query".into(), self.pane == Pane::Query)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pane = Pane::Query;
                        cx.notify();
                    })),
            )
            .child(heading("Message broker"))
            .child(
                row("mqtt".into(), "radio-tower", "MQTT console".into(), self.pane == Pane::Mqtt)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pane = Pane::Mqtt;
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .p_4()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        "PostgreSQL, MySQL, MongoDB and Redis need a network connection a web page cannot open. They work in the desktop app, with the CLI and an MCP server for AI agents.",
                    ),
            )
    }

    fn status_bar(&self, text: Option<(String, bool)>, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        h_flex()
            .h(px(32.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .child(match text {
                Some((t, true)) => div().flex_1().truncate().text_color(theme.danger).child(t),
                Some((t, false)) => div()
                    .flex_1()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(t),
                None => div().flex_1(),
            })
    }

    fn render_table(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pages = self.total.div_ceil(PAGE).max(1);
        let header = h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(icon("table").size_4().text_color(theme.muted_foreground))
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .font_weight(FontWeight::MEDIUM)
                    .child(crate::demo::TABLES[ix]),
            );
        let pager = h_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("Page {} of {pages}", self.page + 1)),
            )
            .child(
                Button::new("prev")
                    .icon(icon("chevron-left"))
                    .ghost()
                    .xsmall()
                    .disabled(self.page == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = this.page.saturating_sub(1);
                        this.load_table(cx);
                    })),
            )
            .child(
                Button::new("next")
                    .icon(icon("chevron-right"))
                    .ghost()
                    .xsmall()
                    .disabled(self.page + 1 >= pages)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page += 1;
                        this.load_table(cx);
                    })),
            );
        v_flex()
            .size_full()
            .child(header)
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.grid)
                        .stripe(true)
                        .bordered(false)
                        .header_row_height(px(GRID_HEADER_H)),
                ),
            )
            .child(self.status_bar(self.status.clone(), cx).child(pager))
            .into_any_element()
    }

    fn render_query(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(44.))
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("run")
                            .icon(icon("play"))
                            .label("Run")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.run_query(cx))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Runs every statement in the editor on the sample database. Writes stay in this tab"),
                    ),
            )
            .child(
                div()
                    .h(px(220.))
                    .flex_none()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Editor::new(&self.editor).bordered(false).h_full()),
            )
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.query_grid)
                        .stripe(true)
                        .bordered(false)
                        .header_row_height(px(GRID_HEADER_H)),
                ),
            )
            .child(self.status_bar(self.query_status.clone(), cx))
            .into_any_element()
    }

    fn render_mqtt(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let connected = self
            .client
            .as_ref()
            .is_some_and(|c| c.inbox.borrow().connected);
        let label = |t: &'static str| div().text_xs().text_color(theme.muted_foreground).child(t);
        let form = v_flex()
            .p_3()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex().gap_1().child(label("Broker (MQTT over WebSocket)")).child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.broker).small()))
                        .child(
                            Button::new("connect")
                                .icon(icon(if self.client.is_some() { "unplug" } else { "plug" }))
                                .label(if self.client.is_some() { "Disconnect" } else { "Connect" })
                                .when(self.client.is_none(), |b| b.primary())
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_broker(cx))),
                        ),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex().flex_1().gap_1().child(label("Subscribe to")).child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(Input::new(&self.sub_filter).small()))
                                .child(
                                    Button::new("sub")
                                        .label("Subscribe")
                                        .small()
                                        .disabled(!connected)
                                        .on_click(cx.listener(|this, _, _, cx| this.subscribe(cx))),
                                ),
                        ),
                    )
                    .child(
                        v_flex().flex_1().gap_1().child(label("Publish")).child(
                            h_flex()
                                .gap_2()
                                .child(div().w(px(180.)).child(Input::new(&self.pub_topic).small()))
                                .child(div().flex_1().child(Input::new(&self.pub_payload).small()))
                                .child(
                                    Button::new("pub")
                                        .icon(icon("send"))
                                        .label("Publish")
                                        .small()
                                        .disabled(!connected)
                                        .on_click(cx.listener(|this, _, _, cx| this.publish(cx))),
                                ),
                        ),
                    ),
            );
        let rows = self.messages.iter().rev().take(200).map(|m| {
            let payload = String::from_utf8_lossy(&m.payload)
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(200)
                .collect::<String>();
            let secs = (m.at_ms / 1000.0) as i64;
            let (h, mi, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
            h_flex()
                .h(px(44.))
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    icon(if m.sent { "arrow-up-right" } else { "arrow-down-left" })
                        .size_3p5()
                        .text_color(if m.sent { theme.primary } else { theme.muted_foreground }),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().truncate().text_sm().child(m.topic.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(format!(
                                            "{h:02}:{mi:02}:{s:02} UTC · QoS {}{} · {} B",
                                            m.qos,
                                            if m.retain { " · retained" } else { "" },
                                            m.payload.len()
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(payload),
                        ),
                )
        });
        let empty = self.messages.is_empty().then(|| {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if connected {
                    "Waiting for messages. Publish one above, or from any MQTT client on the same broker"
                } else {
                    "Connect to a public broker that speaks MQTT over WebSocket (wss://), then subscribe and publish. Messages go through that broker, so keep them harmless"
                })
        });
        v_flex()
            .size_full()
            .child(form)
            .child(
                div()
                    .id("mqtt-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(empty)
                    .children(rows),
            )
            .child(self.status_bar(self.mqtt_note.clone(), cx))
            .into_any_element()
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = if let Some(e) = &self.startup_error {
            div()
                .p_6()
                .text_color(theme.danger)
                .child(format!("The sample database did not open: {e}"))
                .into_any_element()
        } else {
            match self.pane {
                Pane::Table(ix) => self.render_table(ix, cx),
                Pane::Query => self.render_query(cx),
                Pane::Mqtt => self.render_mqtt(cx),
            }
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(self.render_top(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().min_w_0().h_full().child(body)),
            )
    }
}
