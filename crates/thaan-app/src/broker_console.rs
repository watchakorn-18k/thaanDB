//! MQTT console (docs/spec/table-view.md T7): one tab per broker connection.
//!
//! Left: *Subscriptions*: add one at the top (filter, QoS); below, *All messages* and each
//! subscription, which narrows the list to what it matches; `x` unsubscribes.
//! Middle: the messages received and sent, newest first, polled from the session's
//! bounded buffer (`broker.messages` after the last id seen), with a direction switch, a
//! search, Pause and Clear. Under them an optional publish panel (topic, payload, QoS,
//! retain). Right: the picked message in full (JSON pretty-printed), with *Copy* and
//! *Publish to this topic*.
//! Subscribe, unsubscribe and publish go through `session.command` (not history).

use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use thaan_core::browse::{BrokerMessage, Subscription};
use thaan_core::cmdline::quote_arg;
use thaan_core::engine::{Engine, SessionId};

/// Messages kept in the tab (the driver keeps its own bounded buffer too).
const KEEP: usize = 2_000;
const POLL: Duration = Duration::from_millis(500);
const ROW_H: f32 = 48.;
/// Characters of a payload shown in a list row.
const PREVIEW: usize = 200;
/// Payload bytes pretty-printed in the detail panel (bigger ones show as received).
const PRETTY_MAX: usize = 64 * 1024;

fn icon(name: &str) -> Icon {
    Icon::empty().path(SharedString::from(format!("icons/{name}.svg")))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    All,
    Received,
    Sent,
}

/// Local time of day, `14:03:07.512`.
fn clock(at_ms: u64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(at_ms as i64)
        .single()
        .map(|t| t.format("%H:%M:%S%.3f").to_string())
        .unwrap_or_default()
}

/// Payload as text when it is UTF-8, else as hex.
fn payload_text(p: &[u8]) -> String {
    match std::str::from_utf8(p) {
        Ok(s) => s.to_string(),
        Err(_) => p
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// The payload for reading: JSON indented, anything else as is.
fn pretty(p: &[u8]) -> String {
    if p.len() <= PRETTY_MAX {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(p) {
            if v.is_object() || v.is_array() {
                if let Ok(s) = serde_json::to_string_pretty(&v) {
                    return s;
                }
            }
        }
    }
    payload_text(p)
}

/// One line for the list: newlines folded, cut at [`PREVIEW`] characters.
fn preview(p: &[u8]) -> String {
    let t = payload_text(p);
    let mut one: String = t
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(PREVIEW)
        .collect();
    if t.chars().count() > PREVIEW {
        one.push('…');
    }
    one
}

/// MQTT topic filter match (`+` one level, `#` the rest; `$` topics only by name).
fn topic_matches(filter: &str, topic: &str) -> bool {
    if topic.starts_with('$') && !filter.starts_with('$') {
        return false;
    }
    let (mut f, mut t) = (filter.split('/'), topic.split('/'));
    loop {
        match (f.next(), t.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(a), Some(b)) if a == b => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// A small muted label above a field.
fn field_label(text: &'static str, color: Hsla) -> Div {
    div().text_xs().text_color(color).child(text)
}

pub struct BrokerConsole {
    engine: Arc<Engine>,
    sid: SessionId,
    read_only: bool,
    subs: Vec<Subscription>,
    /// The subscription the list is narrowed to (`None`: all messages).
    only: Option<String>,
    sub_filter: Entity<InputState>,
    sub_qos: u8,
    messages: VecDeque<BrokerMessage>,
    last_id: u64,
    direction: Direction,
    search: Entity<InputState>,
    paused: bool,
    selected: Option<u64>,
    show_publish: bool,
    topic: Entity<InputState>,
    payload: Entity<EditorState>,
    qos: u8,
    retain: bool,
    /// The last action's outcome (`true` for an error).
    note: Option<(String, bool)>,
    busy: bool,
    error: Option<String>,
    list: UniformListScrollHandle,
}

impl BrokerConsole {
    pub fn new(
        engine: Arc<Engine>,
        sid: SessionId,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub_filter = cx.new(|cx| InputState::new(window, cx).placeholder("sensors/#"));
        cx.subscribe_in(
            &sub_filter,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.subscribe(window, cx);
                }
            },
        )
        .detach();
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search topic or payload"));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let topic = cx.new(|cx| InputState::new(window, cx).placeholder("sensors/temperature"));
        cx.subscribe_in(&topic, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.publish(cx);
            }
        })
        .detach();
        let payload = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                .soft_wrap(true)
        });
        let this = Self {
            engine,
            sid,
            read_only,
            subs: Vec::new(),
            only: None,
            sub_filter,
            sub_qos: 0,
            messages: VecDeque::new(),
            last_id: 0,
            direction: Direction::All,
            search,
            paused: false,
            selected: None,
            show_publish: false,
            topic,
            payload,
            qos: 0,
            retain: false,
            note: None,
            busy: false,
            error: None,
            list: UniformListScrollHandle::new(),
        };
        this.refresh_subs(cx);
        // Poll the session's buffer; the loop ends when the tab (this entity) is gone.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let Ok(Some((engine, sid, since))) = this.update(cx, |c, _| {
                (!c.paused).then(|| (c.engine.clone(), c.sid, c.last_id))
            }) else {
                if this.upgrade().is_none() {
                    break;
                }
                continue;
            };
            let res = cx
                .background_spawn(async move { engine.messages_since(sid, since, 500) })
                .await;
            if this.update(cx, |c, cx| c.take_messages(res, cx)).is_err() {
                break;
            }
        })
        .detach();
        this
    }

    fn take_messages(
        &mut self,
        res: thaan_core::Result<Vec<BrokerMessage>>,
        cx: &mut Context<Self>,
    ) {
        match res {
            Ok(new) if new.is_empty() => {}
            Ok(new) => {
                for m in new {
                    if m.id <= self.last_id {
                        continue;
                    }
                    self.last_id = m.id;
                    self.messages.push_front(m);
                }
                while self.messages.len() > KEEP {
                    self.messages.pop_back();
                }
                self.error = None;
                cx.notify();
            }
            Err(e) => {
                let e = e.to_string();
                if self.error.as_deref() != Some(e.as_str()) {
                    self.error = Some(e);
                    cx.notify();
                }
            }
        }
    }

    fn refresh_subs(&self, cx: &mut Context<Self>) {
        let (engine, sid) = (self.engine.clone(), self.sid);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.subscriptions(sid) })
                .await;
            this.update(cx, |c, cx| {
                if let Ok(s) = res {
                    if c.only
                        .as_ref()
                        .is_some_and(|o| !s.iter().any(|x| &x.filter == o))
                    {
                        c.only = None;
                    }
                    c.subs = s;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Runs one driver command off the UI thread, then reloads the subscriptions.
    fn run(&mut self, command: String, done: &'static str, cx: &mut Context<Self>) {
        let (engine, sid) = (self.engine.clone(), self.sid);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.driver_command(sid, &command) })
                .await;
            this.update(cx, |c, cx| {
                c.busy = false;
                c.note = Some(match res {
                    Ok(rs) => (
                        rs.rows
                            .first()
                            .and_then(|r| r.first())
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| done.to_string()),
                        false,
                    ),
                    Err(e) => (e.to_string(), true),
                });
                c.refresh_subs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn subscribe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = self.sub_filter.read(cx).value().trim().to_string();
        if f.is_empty() {
            return;
        }
        self.sub_filter
            .update(cx, |s, cx| s.set_value("", window, cx));
        let cmd = format!("SUBSCRIBE {} QOS {}", quote_arg(&f), self.sub_qos);
        self.run(cmd, "subscribed", cx);
    }

    fn unsubscribe(&mut self, filter: String, cx: &mut Context<Self>) {
        if self.only.as_deref() == Some(filter.as_str()) {
            self.only = None;
        }
        self.run(
            format!("UNSUBSCRIBE {}", quote_arg(&filter)),
            "unsubscribed",
            cx,
        );
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let topic = self.topic.read(cx).value().trim().to_string();
        if topic.is_empty() {
            self.note = Some(("Type a topic to publish to".into(), true));
            cx.notify();
            return;
        }
        let payload = self.payload.read(cx).value().to_string();
        let mut cmd = format!(
            "PUBLISH {} {} QOS {}",
            quote_arg(&topic),
            quote_arg(&payload),
            self.qos
        );
        if self.retain {
            cmd.push_str(" RETAIN");
        }
        self.run(cmd, "published", cx);
    }

    /// Opens the publish panel with the picked message's topic filled in.
    fn reply_to(&mut self, topic: String, window: &mut Window, cx: &mut Context<Self>) {
        self.show_publish = true;
        self.topic
            .update(cx, |s, cx| s.set_value(topic, window, cx));
        cx.notify();
    }

    fn visible(&self, cx: &App) -> Vec<usize> {
        let q = self.search.read(cx).value().to_lowercase();
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, m)| match self.direction {
                Direction::All => true,
                Direction::Received => !m.sent,
                Direction::Sent => m.sent,
            })
            .filter(|(_, m)| {
                self.only
                    .as_deref()
                    .is_none_or(|f| topic_matches(f, &m.topic))
            })
            .filter(|(_, m)| {
                q.is_empty()
                    || m.topic.to_lowercase().contains(&q)
                    || payload_text(&m.payload).to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn qos_group(
        id: &'static str,
        value: u8,
        on: impl Fn(&mut Self, u8) + 'static,
        cx: &mut Context<Self>,
    ) -> ButtonGroup {
        ButtonGroup::new(id)
            .outline()
            .xsmall()
            .child(
                Button::new(SharedString::from(format!("{id}-0")))
                    .label("QoS 0")
                    .selected(value == 0),
            )
            .child(
                Button::new(SharedString::from(format!("{id}-1")))
                    .label("QoS 1")
                    .selected(value == 1),
            )
            .on_click(cx.listener(move |this, clicks: &Vec<usize>, _, cx| {
                if let Some(&i) = clicks.first() {
                    on(this, i as u8);
                    cx.notify();
                }
            }))
    }

    // ---- render ---------------------------------------------------------------------

    fn render_subs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row_bg = |on: bool, d: Stateful<Div>| {
            d.when(on, |d| d.bg(theme.foreground.opacity(0.06)))
                .hover(|d| d.bg(theme.foreground.opacity(0.04)))
        };
        let meta = |t: String| {
            div()
                .flex_none()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let all = row_bg(
            self.only.is_none(),
            h_flex()
                .id("sub-all")
                .w_full()
                .h(px(32.))
                .px_3()
                .gap_2()
                .text_sm()
                .cursor_pointer(),
        )
        .child(icon("inbox").size_3p5().text_color(theme.muted_foreground))
        .child(div().flex_1().child("All messages"))
        .child(meta(self.messages.len().to_string()))
        .on_click(cx.listener(|this, _, _, cx| {
            this.only = None;
            cx.notify();
        }));
        let subs: Vec<_> = self
            .subs
            .iter()
            .map(|s| {
                let f = s.filter.clone();
                let f2 = s.filter.clone();
                let n = self
                    .messages
                    .iter()
                    .filter(|m| !m.sent && topic_matches(&s.filter, &m.topic))
                    .count();
                row_bg(
                    self.only.as_deref() == Some(s.filter.as_str()),
                    h_flex()
                        .id(SharedString::from(format!("sub-{}", s.filter)))
                        .w_full()
                        .h(px(32.))
                        .pl_3()
                        .pr_1()
                        .gap_2()
                        .text_sm()
                        .cursor_pointer(),
                )
                .child(icon("radio").size_3p5().text_color(theme.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .child(s.filter.clone()),
                )
                .child(meta(format!("QoS {} · {n}", s.qos)))
                .child(
                    Button::new(SharedString::from(format!("unsub-{}", s.filter)))
                        .icon(icon("x"))
                        .ghost()
                        .xsmall()
                        .tooltip("Unsubscribe")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.unsubscribe(f.clone(), cx)),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.only = Some(f2.clone());
                    cx.notify();
                }))
            })
            .collect();
        let add = v_flex()
            .p_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(field_label(
                "Subscribe to a topic filter",
                theme.muted_foreground,
            ))
            .child(Input::new(&self.sub_filter).small())
            .child(
                h_flex()
                    .gap_2()
                    .child(Self::qos_group(
                        "sub-qos",
                        self.sub_qos,
                        |t, q| t.sub_qos = q,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("subscribe")
                            .icon(icon("plus"))
                            .label("Subscribe")
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.subscribe(window, cx)),
                            ),
                    ),
            );
        let empty = self.subs.is_empty().then(|| {
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No subscriptions yet. Messages arrive once you subscribe")
        });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(44.))
                    .flex_none()
                    .px_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Subscriptions"),
                    )
                    .child(meta(self.subs.len().to_string())),
            )
            .child(add)
            .child(
                v_flex()
                    .flex_1()
                    .py_1()
                    .child(all)
                    .children(subs)
                    .children(empty),
            )
    }

    fn render_toolbar(&mut self, shown: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dir = self.direction;
        h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                ButtonGroup::new("direction")
                    .outline()
                    .xsmall()
                    .child(
                        Button::new("dir-all")
                            .label("All")
                            .selected(dir == Direction::All),
                    )
                    .child(
                        Button::new("dir-in")
                            .label("Received")
                            .selected(dir == Direction::Received),
                    )
                    .child(
                        Button::new("dir-out")
                            .label("Sent")
                            .selected(dir == Direction::Sent),
                    )
                    .on_click(cx.listener(|this, clicks: &Vec<usize>, _, cx| {
                        this.direction = match clicks.first() {
                            Some(1) => Direction::Received,
                            Some(2) => Direction::Sent,
                            _ => Direction::All,
                        };
                        cx.notify();
                    })),
            )
            .child(
                div().flex_1().min_w(px(180.)).max_w(px(320.)).child(
                    Input::new(&self.search)
                        .small()
                        .prefix(icon("search").size_3p5()),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .flex_none()
                    .child(format!("{shown} messages")),
            )
            .child(
                Button::new("pause")
                    .icon(icon(if self.paused { "play" } else { "pause" }))
                    .label(if self.paused { "Resume" } else { "Pause" })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.paused = !this.paused;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("clear")
                    .icon(icon("eraser"))
                    .label("Clear")
                    .ghost()
                    .xsmall()
                    .tooltip("Empty this list (the broker is not changed)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.messages.clear();
                        this.selected = None;
                        cx.notify();
                    })),
            )
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("toggle-publish")
                        .icon(icon("send"))
                        .label("Publish")
                        .ghost()
                        .xsmall()
                        .selected(self.show_publish)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_publish = !this.show_publish;
                            cx.notify();
                        })),
                )
            })
    }

    fn render_list(&mut self, shown: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if shown.is_empty() {
            let note = match (&self.error, self.messages.is_empty(), self.subs.is_empty()) {
                (Some(e), _, _) => e.clone(),
                (None, true, true) => {
                    "Subscribe to a topic filter on the left to see messages".into()
                }
                (None, true, false) => "Waiting for messages…".into(),
                (None, false, _) => "No messages match".into(),
            };
            return div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(if self.error.is_some() {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(note)
                .into_any_element();
        }
        let view = cx.entity().downgrade();
        let selected = self.selected;
        uniform_list("messages", shown.len(), move |range, _, cx| {
            let Some(view) = view.upgrade() else {
                return Vec::new();
            };
            let c = view.read(cx);
            let theme = cx.theme();
            range
                .filter_map(|i| {
                    let m = c.messages.get(*shown.get(i)?)?;
                    let id = m.id;
                    let on = selected == Some(id);
                    let v = view.downgrade();
                    let meta = format!(
                        "{}{} · QoS {} · {} B",
                        clock(m.at_ms),
                        if m.retain { " · retained" } else { "" },
                        m.qos,
                        m.size
                    );
                    let top = h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(m.topic.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(meta),
                        );
                    let body = div()
                        .truncate()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(preview(&m.payload));
                    let arrow = icon(if m.sent {
                        "arrow-up-right"
                    } else {
                        "arrow-down-left"
                    })
                    .size_3p5()
                    .flex_none()
                    .text_color(if m.sent {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    });
                    Some(
                        h_flex()
                            .id(("msg", id as usize))
                            .w_full()
                            .h(px(ROW_H))
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .when(on, |d| d.bg(theme.foreground.opacity(0.06)))
                            .hover(|d| d.bg(theme.foreground.opacity(0.04)))
                            .child(arrow)
                            .child(v_flex().flex_1().min_w_0().child(top).child(body))
                            .on_click(move |_, _, cx| {
                                v.update(cx, |c, cx| {
                                    c.selected = if c.selected == Some(id) {
                                        None
                                    } else {
                                        Some(id)
                                    };
                                    cx.notify();
                                })
                                .ok();
                            }),
                    )
                })
                .collect()
        })
        .track_scroll(&self.list)
        .flex_1()
        .into_any_element()
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let m = self
            .selected
            .and_then(|id| self.messages.iter().find(|m| m.id == id))?
            .clone();
        let full = pretty(&m.payload);
        let copy = payload_text(&m.payload);
        let topic = m.topic.clone();
        let mut meta = vec![
            clock(m.at_ms),
            if m.sent { "sent" } else { "received" }.to_string(),
            format!("QoS {}", m.qos),
            format!("{} B", m.size),
        ];
        if m.retain {
            meta.push("retained".into());
        }
        if m.size > m.payload.len() {
            meta.push(format!("first {} B kept", m.payload.len()));
        }
        let header = h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(m.topic.clone()),
            )
            .child(
                Button::new("close-detail")
                    .icon(icon("x"))
                    .ghost()
                    .xsmall()
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = None;
                        cx.notify();
                    })),
            );
        let actions = h_flex()
            .p_3()
            .gap_2()
            .child(
                Button::new("copy-payload")
                    .icon(icon("clipboard"))
                    .label("Copy")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    }),
            )
            .when(!self.read_only, |h| {
                h.child(
                    Button::new("reply")
                        .icon(icon("send"))
                        .label("Publish to this topic")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reply_to(topic.clone(), window, cx)
                        })),
                )
            });
        Some(
            v_flex()
                .w(px(360.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(theme.border)
                .child(header)
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(meta.join(" · ")),
                )
                .child(
                    div()
                        .id("payload-full")
                        .flex_1()
                        .mx_3()
                        .p_3()
                        .rounded(px(6.))
                        .bg(theme.foreground.opacity(0.03))
                        .border_1()
                        .border_color(theme.border)
                        .overflow_y_scroll()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .child(full),
                )
                .child(actions)
                .into_any_element(),
        )
    }

    fn render_publish(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let note = div().flex_1().min_w_0().truncate().text_xs().when_some(
            self.note.clone(),
            |d, (n, err)| {
                d.text_color(if err {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(n)
            },
        );
        v_flex()
            .flex_none()
            .p_3()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Publish"),
                    )
                    .child(
                        Button::new("hide-publish")
                            .icon(icon("chevron-down"))
                            .ghost()
                            .xsmall()
                            .tooltip("Hide")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_publish = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(field_label("Topic", theme.muted_foreground))
                    .child(Input::new(&self.topic).small()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(field_label("Payload", theme.muted_foreground))
                    .child(
                        div()
                            .h(px(96.))
                            .border_1()
                            .border_color(theme.input)
                            .rounded(px(6.))
                            .overflow_hidden()
                            .child(Editor::new(&self.payload).bordered(false).h_full()),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Self::qos_group("pub-qos", self.qos, |t, q| t.qos = q, cx))
                    .child(
                        Checkbox::new("retain")
                            .label("Retain")
                            .checked(self.retain)
                            .on_click(cx.listener(|this, v: &bool, _, cx| {
                                this.retain = *v;
                                cx.notify();
                            })),
                    )
                    .child(note)
                    .child(
                        Button::new("publish")
                            .icon(icon("send"))
                            .label(if self.busy {
                                "Publishing…"
                            } else {
                                "Publish"
                            })
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.publish(cx))),
                    ),
            )
    }
}

impl Render for BrokerConsole {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let shown = self.visible(cx);
        let toolbar = self.render_toolbar(shown.len(), cx);
        let list = self.render_list(shown, cx);
        let detail = self.render_detail(cx);
        let publish = (self.show_publish && !self.read_only).then(|| self.render_publish(cx));
        let subs = self.render_subs(cx);
        h_flex()
            .size_full()
            .child(
                div()
                    .w(px(280.))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(border)
                    .child(subs),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(toolbar)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(v_flex().flex_1().min_w_0().h_full().child(list))
                            .children(detail),
                    )
                    .children(publish),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` attribute in through `gpui_kit::*`.
    use super::{pretty, preview, topic_matches, PREVIEW};

    #[test]
    fn previews_fold_lines_and_cut() {
        assert_eq!(preview(b"a\nb"), "a b");
        assert_eq!(preview(&[0xff, 0x01]), "ff 01");
        let long = vec![b'x'; PREVIEW + 5];
        assert!(preview(&long).ends_with('…'));
    }

    #[test]
    fn json_payloads_are_indented() {
        assert_eq!(pretty(br#"{"a":1}"#), "{\n  \"a\": 1\n}");
        assert_eq!(pretty(b"21.5"), "21.5");
    }

    #[test]
    fn filters_match_topics() {
        assert!(topic_matches("a/#", "a/b/c"));
        assert!(topic_matches("a/+/c", "a/b/c"));
        assert!(!topic_matches("#", "$SYS/x"));
    }
}
