//! An MQTT 3.1.1 client over the browser's WebSocket (subprotocol `mqtt`), for brokers that
//! accept MQTT over WebSocket. Packets are the desktop driver's (`thaan_driver_mqtt::packet`);
//! only the transport differs. Events land in an `Inbox` the view drains on a timer.

use std::cell::RefCell;
use std::io::Cursor;
use std::rc::Rc;
use thaan_driver_mqtt::packet::{self, Incoming};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{BinaryType, CloseEvent, MessageEvent, WebSocket};

/// Messages kept in the inbox between two drains.
const INBOX_CAP: usize = 2_000;

#[derive(Clone, Debug)]
pub struct Message {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
    pub sent: bool,
    /// Milliseconds since the Unix epoch (browser clock).
    pub at_ms: f64,
}

#[derive(Default)]
pub struct Inbox {
    pub connected: bool,
    pub closed: bool,
    pub error: Option<String>,
    pub messages: Vec<Message>,
    /// Bytes received but not yet a whole packet.
    buf: Vec<u8>,
}

pub struct Client {
    ws: WebSocket,
    pub inbox: Rc<RefCell<Inbox>>,
    next_id: u16,
    // Kept alive for as long as the socket.
    _on_open: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::Event)>,
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn send(ws: &WebSocket, bytes: &[u8]) -> Result<(), String> {
    ws.send_with_u8_array(bytes)
        .map_err(|e| format!("send failed: {e:?}"))
}

impl Client {
    /// Opens the socket; CONNECT goes out when it opens, CONNACK marks it connected.
    pub fn connect(url: &str) -> Result<Self, String> {
        if !(url.starts_with("ws://") || url.starts_with("wss://")) {
            return Err("the broker URL starts with wss:// (or ws:// for a local broker)".into());
        }
        let ws = WebSocket::new_with_str(url, "mqtt").map_err(|e| format!("{e:?}"))?;
        ws.set_binary_type(BinaryType::Arraybuffer);
        let inbox = Rc::new(RefCell::new(Inbox::default()));
        let client_id = format!("thaan-web-{:x}", (js_sys::Math::random() * 1e12) as u64);

        let on_open = {
            let ws = ws.clone();
            let inbox = inbox.clone();
            Closure::<dyn FnMut()>::new(move || {
                let connect = packet::connect(&packet::Connect {
                    client_id: &client_id,
                    user: None,
                    password: None,
                    keep_alive: 60,
                });
                if let Err(e) = send(&ws, &connect) {
                    inbox.borrow_mut().error = Some(e);
                }
            })
        };
        let on_message = {
            let ws = ws.clone();
            let inbox = inbox.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                let Ok(buf) = ev.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return;
                };
                let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                let mut inbox = inbox.borrow_mut();
                inbox.buf.extend_from_slice(&bytes);
                // A frame can hold part of a packet or several: take the whole ones.
                loop {
                    let mut cur = Cursor::new(&inbox.buf[..]);
                    match packet::read(&mut cur) {
                        Ok(p) => {
                            let used = cur.position() as usize;
                            inbox.buf.drain(..used);
                            handle(&ws, &mut inbox, p);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(e) => {
                            inbox.error = Some(format!("bad packet from the broker: {e}"));
                            inbox.buf.clear();
                            break;
                        }
                    }
                }
            })
        };
        let on_close = {
            let inbox = inbox.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |ev: CloseEvent| {
                let mut inbox = inbox.borrow_mut();
                if inbox.error.is_none() && !inbox.connected {
                    inbox.error = Some(format!(
                        "the broker closed the connection (code {}). Check the URL and that it accepts MQTT over WebSocket",
                        ev.code()
                    ));
                }
                inbox.connected = false;
                inbox.closed = true;
            })
        };
        let on_error = {
            let inbox = inbox.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                let mut inbox = inbox.borrow_mut();
                if inbox.error.is_none() {
                    inbox.error = Some("could not reach the broker".into());
                }
            })
        };
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        Ok(Self {
            ws,
            inbox,
            next_id: 1,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_error: on_error,
        })
    }

    fn packet_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    pub fn subscribe(&mut self, filter: &str, qos: u8) -> Result<(), String> {
        let id = self.packet_id();
        send(&self.ws, &packet::subscribe(id, &[(filter, qos.min(1))]))
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: u8, retain: bool) -> Result<(), String> {
        if topic.is_empty() || topic.contains(['+', '#']) {
            return Err("a topic to publish to has no wildcards".into());
        }
        let qos = qos.min(1);
        let id = if qos > 0 { self.packet_id() } else { 0 };
        send(&self.ws, &packet::publish(topic, payload, qos, retain, id))?;
        let mut inbox = self.inbox.borrow_mut();
        inbox.messages.push(Message {
            topic: topic.to_string(),
            payload: payload.to_vec(),
            qos,
            retain,
            sent: true,
            at_ms: now_ms(),
        });
        Ok(())
    }

    pub fn ping(&self) {
        send(&self.ws, &packet::pingreq()).ok();
    }

    pub fn close(&self) {
        send(&self.ws, &packet::disconnect()).ok();
        self.ws.close().ok();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        self.ws.close().ok();
    }
}

fn handle(ws: &WebSocket, inbox: &mut Inbox, p: Incoming) {
    match p {
        Incoming::ConnAck { code: 0, .. } => inbox.connected = true,
        Incoming::ConnAck { code, .. } => {
            inbox.error = Some(format!("the broker refused: {}", packet::connack_error(code)))
        }
        Incoming::Publish {
            topic,
            payload,
            qos,
            retain,
            packet_id,
        } => {
            if let (1, Some(id)) = (qos, packet_id) {
                send(ws, &packet::puback(id)).ok();
            }
            inbox.messages.push(Message {
                topic,
                payload,
                qos,
                retain,
                sent: false,
                at_ms: now_ms(),
            });
            if inbox.messages.len() > INBOX_CAP {
                let extra = inbox.messages.len() - INBOX_CAP;
                inbox.messages.drain(..extra);
            }
        }
        Incoming::SubAck { codes, .. } if codes.first() == Some(&0x80) => {
            inbox.error = Some("the broker refused the subscription".into())
        }
        _ => {}
    }
}
