//! MQTT console (docs/spec/table-view.md T7): one tab per broker connection.
//! Left: the subscriptions this session holds, and a box to add one. Right: the messages
//! received and sent, newest first, polled from the session's bounded buffer
//! (`broker.messages` after the last id seen), with a direction switch, a payload search
//! and Pause; under them the picked message in full, and a publish panel.
//! Subscribe, unsubscribe and publish go through `session.command` (not history).

use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
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

pub struct BrokerConsole {
    engine: Arc<Engine>,
    sid: SessionId,
    read_only: bool,
    subs: Vec<Subscription>,
    sub_filter: Entity<InputState>,
    sub_qos: u8,
    messages: VecDeque<BrokerMessage>,
    last_id: u64,
    direction: Direction,
    search: Entity<InputState>,
    paused: bool,
    selected: Option<u64>,
    topic: Entity<InputState>,
    payload: Entity<EditorState>,
    qos: u8,
    retain: bool,
    /// The last action's outcome, shown above Publish (`true` for an error).
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
        let sub_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Topic filter, e.g. sensors/#"));
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search payloads"));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let topic =
            cx.new(|cx| InputState::new(window, cx).placeholder("Topic, e.g. sensors/temperature"));
        let payload = cx.new(|cx| EditorState::new(window, cx).line_number(false));
        let this = Self {
            engine,
            sid,
            read_only,
            subs: Vec::new(),
            sub_filter,
            sub_qos: 0,
            messages: VecDeque::new(),
            last_id: 0,
            direction: Direction::All,
            search,
            paused: false,
            selected: None,
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
                q.is_empty()
                    || m.topic.to_lowercase().contains(&q)
                    || payload_text(&m.payload).to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn render_subs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.subs.iter().map(|s| {
            let f = s.filter.clone();
            h_flex()
                .h(px(32.))
                .px_3()
                .gap_2()
                .text_sm()
                .child(icon("radio").size_3p5().text_color(theme.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .child(s.filter.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("QoS {}", s.qos)),
                )
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
        });
        let empty = self.subs.is_empty().then(|| {
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No subscriptions. Add a topic filter below")
        });
        let qos_btn = |q: u8, cx: &mut Context<Self>| {
            Button::new(SharedString::from(format!("sub-qos-{q}")))
                .label(format!("QoS {q}"))
                .ghost()
                .xsmall()
                .selected(self.sub_qos == q)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sub_qos = q;
                    cx.notify();
                }))
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(40.))
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
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(self.subs.len().to_string()),
                    ),
            )
            .child(v_flex().flex_1().py_1().children(rows).children(empty))
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.sub_filter).small())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(qos_btn(0, cx))
                            .child(qos_btn(1, cx))
                            .child(div().flex_1())
                            .child(
                                Button::new("subscribe")
                                    .label("Subscribe")
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.subscribe(window, cx)
                                    })),
                            ),
                    ),
            )
    }

    fn render_messages(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let shown = self.visible(cx);
        let dir_btn = |d: Direction, label: &'static str, cx: &mut Context<Self>| {
            Button::new(label)
                .label(label)
                .ghost()
                .xsmall()
                .selected(self.direction == d)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.direction = d;
                    cx.notify();
                }))
        };
        let bar = h_flex()
            .h(px(40.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_1()
                    .child(dir_btn(Direction::All, "All", cx))
                    .child(dir_btn(Direction::Received, "Received", cx))
                    .child(dir_btn(Direction::Sent, "Sent", cx)),
            )
            .child(
                div().flex_1().max_w(px(320.)).child(
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
                    .child(format!("{} messages", shown.len())),
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
                    .ghost()
                    .xsmall()
                    .tooltip("Clear the list (the broker is not changed)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.messages.clear();
                        this.selected = None;
                        cx.notify();
                    })),
            );
        let list = if shown.is_empty() {
            let note = match (&self.error, self.messages.is_empty(), self.subs.is_empty()) {
                (Some(e), _, _) => e.clone(),
                (None, true, true) => "Subscribe to a topic filter to see messages".into(),
                (None, true, false) => "Waiting for messages…".into(),
                (None, false, _) => "No messages match".into(),
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
                                .child(
                                    icon(if m.sent {
                                        "arrow-up-right"
                                    } else {
                                        "arrow-down-left"
                                    })
                                    .size_3p5()
                                    .flex_none()
                                    .text_color(theme.muted_foreground),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            h_flex()
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
                                                ),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .font_family(theme.mono_font_family.clone())
                                                .text_color(theme.muted_foreground)
                                                .child(preview(&m.payload)),
                                        ),
                                )
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
        };
        let detail = self
            .selected
            .and_then(|id| self.messages.iter().find(|m| m.id == id))
            .map(|m| {
                let full = payload_text(&m.payload);
                let copy = full.clone();
                v_flex()
                    .flex_none()
                    .max_h(px(200.))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .h(px(32.))
                            .px_3()
                            .gap_2()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(div().flex_1().truncate().child(format!(
                                "{} · {}{}",
                                m.topic,
                                clock(m.at_ms),
                                if m.size > m.payload.len() {
                                    format!(" · first {} of {} bytes", m.payload.len(), m.size)
                                } else {
                                    String::new()
                                }
                            )))
                            .child(
                                Button::new("copy-payload")
                                    .icon(icon("clipboard"))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Copy payload")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("payload-full")
                            .flex_1()
                            .px_3()
                            .pb_2()
                            .overflow_y_scroll()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .child(full),
                    )
            });
        v_flex()
            .size_full()
            .child(bar)
            .child(list)
            .children(detail)
            .when(!self.read_only, |d| d.child(self.render_publish(cx)))
    }

    fn render_publish(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let qos_btn = |q: u8, cx: &mut Context<Self>| {
            Button::new(SharedString::from(format!("pub-qos-{q}")))
                .label(format!("QoS {q}"))
                .ghost()
                .xsmall()
                .selected(self.qos == q)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.qos = q;
                    cx.notify();
                }))
        };
        v_flex()
            .flex_none()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Publish"),
                    )
                    .child(div().flex_1())
                    .child(qos_btn(0, cx))
                    .child(qos_btn(1, cx))
                    .child(
                        Checkbox::new("retain")
                            .label("Retain")
                            .checked(self.retain)
                            .on_click(cx.listener(|this, v: &bool, _, cx| {
                                this.retain = *v;
                                cx.notify();
                            })),
                    ),
            )
            .child(Input::new(&self.topic).small())
            .child(
                div()
                    .h(px(88.))
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.))
                    .overflow_hidden()
                    .child(Editor::new(&self.payload).bordered(false).h_full()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().truncate().text_xs().when_some(
                        self.note.clone(),
                        |d, (n, err)| {
                            d.text_color(if err {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .child(n)
                        },
                    ))
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
        h_flex()
            .size_full()
            .child(
                div()
                    .w(px(280.))
                    .flex_none()
                    .h_full()
                    .border_r_1()
                    .border_color(border)
                    .child(self.render_subs(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_messages(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` attribute in through `gpui_kit::*`.
    use super::{preview, PREVIEW};

    #[test]
    fn previews_fold_lines_and_cut() {
        assert_eq!(preview(b"a\nb"), "a b");
        assert_eq!(preview(&[0xff, 0x01]), "ff 01");
        let long = vec![b'x'; PREVIEW + 5];
        assert!(preview(&long).ends_with('…'));
    }
}
