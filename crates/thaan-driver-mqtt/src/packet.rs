//! MQTT 3.1.1 control packets, written from the OASIS standard
//! (<https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/mqtt-v3.1.1.html>). Only what a client
//! needs: CONNECT, PUBLISH, PUBACK, SUBSCRIBE, UNSUBSCRIBE, PINGREQ, DISCONNECT out;
//! CONNACK, PUBLISH, PUBACK, SUBACK, UNSUBACK, PINGRESP in.

use std::io::{self, Read};

/// Remaining Length is at most four bytes (268,435,455). We refuse anything above this,
/// so one packet cannot make us allocate without bound.
pub const MAX_PACKET: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    ConnAck {
        session_present: bool,
        code: u8,
    },
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
        packet_id: Option<u16>,
    },
    PubAck(u16),
    PubRec(u16),
    PubRel(u16),
    PubComp(u16),
    SubAck {
        packet_id: u16,
        codes: Vec<u8>,
    },
    UnsubAck(u16),
    PingResp,
    Other(u8),
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn push_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn packet(first: u8, body: Vec<u8>) -> Vec<u8> {
    let mut out = vec![first];
    let mut n = body.len();
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out.extend(body);
    out
}

pub struct Connect<'a> {
    pub client_id: &'a str,
    pub user: Option<&'a str>,
    pub password: Option<&'a str>,
    pub keep_alive: u16,
}

pub fn connect(c: &Connect) -> Vec<u8> {
    let mut b = Vec::new();
    push_str(&mut b, b"MQTT");
    b.push(4); // protocol level 3.1.1
    let mut flags = 0x02; // clean session
    if c.user.is_some() {
        flags |= 0x80;
    }
    if c.password.is_some() {
        flags |= 0x40;
    }
    b.push(flags);
    b.extend_from_slice(&c.keep_alive.to_be_bytes());
    push_str(&mut b, c.client_id.as_bytes());
    if let Some(u) = c.user {
        push_str(&mut b, u.as_bytes());
    }
    if let Some(p) = c.password {
        push_str(&mut b, p.as_bytes());
    }
    packet(0x10, b)
}

pub fn publish(topic: &str, payload: &[u8], qos: u8, retain: bool, packet_id: u16) -> Vec<u8> {
    let mut b = Vec::with_capacity(topic.len() + payload.len() + 4);
    push_str(&mut b, topic.as_bytes());
    if qos > 0 {
        b.extend_from_slice(&packet_id.to_be_bytes());
    }
    b.extend_from_slice(payload);
    packet(0x30 | (qos << 1) | u8::from(retain), b)
}

pub fn puback(packet_id: u16) -> Vec<u8> {
    packet(0x40, packet_id.to_be_bytes().to_vec())
}

pub fn pubrec(packet_id: u16) -> Vec<u8> {
    packet(0x50, packet_id.to_be_bytes().to_vec())
}

pub fn pubrel(packet_id: u16) -> Vec<u8> {
    packet(0x62, packet_id.to_be_bytes().to_vec())
}

pub fn pubcomp(packet_id: u16) -> Vec<u8> {
    packet(0x70, packet_id.to_be_bytes().to_vec())
}

pub fn subscribe(packet_id: u16, filters: &[(&str, u8)]) -> Vec<u8> {
    let mut b = packet_id.to_be_bytes().to_vec();
    for (f, qos) in filters {
        push_str(&mut b, f.as_bytes());
        b.push(*qos);
    }
    packet(0x82, b)
}

pub fn unsubscribe(packet_id: u16, filters: &[&str]) -> Vec<u8> {
    let mut b = packet_id.to_be_bytes().to_vec();
    for f in filters {
        push_str(&mut b, f.as_bytes());
    }
    packet(0xA2, b)
}

pub fn pingreq() -> Vec<u8> {
    vec![0xC0, 0]
}

pub fn disconnect() -> Vec<u8> {
    vec![0xE0, 0]
}

fn u16_at(b: &[u8], i: usize) -> io::Result<u16> {
    b.get(i..i + 2)
        .map(|x| u16::from_be_bytes([x[0], x[1]]))
        .ok_or_else(|| bad("packet too short"))
}

/// Reads one packet. Blocks until it is complete.
pub fn read<R: Read>(r: &mut R) -> io::Result<Incoming> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)?;
    let mut len = 0usize;
    let mut mult = 1usize;
    for i in 0..4 {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        len += (b[0] & 0x7f) as usize * mult;
        if b[0] & 0x80 == 0 {
            break;
        }
        if i == 3 {
            return Err(bad("bad remaining length"));
        }
        mult *= 128;
    }
    if len > MAX_PACKET {
        return Err(bad("packet too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let kind = first[0] >> 4;
    let flags = first[0] & 0x0f;
    Ok(match kind {
        2 => Incoming::ConnAck {
            session_present: body.first().is_some_and(|b| b & 1 == 1),
            code: *body.get(1).ok_or_else(|| bad("short CONNACK"))?,
        },
        3 => {
            let qos = (flags >> 1) & 3;
            let tlen = u16_at(&body, 0)? as usize;
            let topic = body.get(2..2 + tlen).ok_or_else(|| bad("short PUBLISH"))?;
            let topic = String::from_utf8(topic.to_vec()).map_err(|_| bad("topic is not UTF-8"))?;
            let mut at = 2 + tlen;
            let packet_id = if qos > 0 {
                let id = u16_at(&body, at)?;
                at += 2;
                Some(id)
            } else {
                None
            };
            Incoming::Publish {
                topic,
                payload: body[at.min(body.len())..].to_vec(),
                qos,
                retain: flags & 1 == 1,
                packet_id,
            }
        }
        4 => Incoming::PubAck(u16_at(&body, 0)?),
        5 => Incoming::PubRec(u16_at(&body, 0)?),
        6 => Incoming::PubRel(u16_at(&body, 0)?),
        7 => Incoming::PubComp(u16_at(&body, 0)?),
        9 => Incoming::SubAck {
            packet_id: u16_at(&body, 0)?,
            codes: body.get(2..).unwrap_or_default().to_vec(),
        },
        11 => Incoming::UnsubAck(u16_at(&body, 0)?),
        13 => Incoming::PingResp,
        k => Incoming::Other(k),
    })
}

/// CONNACK return codes (3.1.1 §3.2.2.3).
pub fn connack_error(code: u8) -> &'static str {
    match code {
        1 => "the broker does not support MQTT 3.1.1",
        2 => "client id rejected",
        3 => "broker unavailable",
        4 => "bad user name or password",
        5 => "not authorised",
        _ => "connection refused",
    }
}

/// Whether `topic` matches the subscription `filter` (`+` one level, `#` the rest).
/// Topics starting with `$` match only filters that name the `$` level (§4.7.2).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    if topic.starts_with('$') && !filter.starts_with('$') {
        return false;
    }
    let mut f = filter.split('/');
    let mut t = topic.split('/');
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_connect() {
        let p = connect(&Connect {
            client_id: "c",
            user: Some("u"),
            password: Some("p"),
            keep_alive: 60,
        });
        assert_eq!(
            p,
            [
                0x10, 19, 0, 4, b'M', b'Q', b'T', b'T', 4, 0xC2, 0, 60, 0, 1, b'c', 0, 1, b'u', 0,
                1, b'p'
            ]
        );
    }

    #[test]
    fn remaining_length_uses_varint() {
        let p = publish("t", &[0u8; 200], 0, false, 0);
        assert_eq!(&p[..3], &[0x30, 0xCB, 0x01]); // 203 = 0x4B | 0x80, 1
        let back = read(&mut &p[..]).unwrap();
        assert!(matches!(back, Incoming::Publish { ref payload, .. } if payload.len() == 200));
    }

    #[test]
    fn decodes_publish_with_id() {
        let p = publish("a/b", b"hi", 1, true, 7);
        assert_eq!(
            read(&mut &p[..]).unwrap(),
            Incoming::Publish {
                topic: "a/b".into(),
                payload: b"hi".to_vec(),
                qos: 1,
                retain: true,
                packet_id: Some(7)
            }
        );
        assert_eq!(
            read(&mut &[0x90, 3, 0, 9, 1][..]).unwrap(),
            Incoming::SubAck {
                packet_id: 9,
                codes: vec![1]
            }
        );
        assert_eq!(read(&mut &[0xD0, 0][..]).unwrap(), Incoming::PingResp);
    }

    #[test]
    fn wildcards() {
        assert!(topic_matches("#", "a/b"));
        assert!(topic_matches("a/#", "a"));
        assert!(topic_matches("a/+/c", "a/b/c"));
        assert!(!topic_matches("a/+", "a/b/c"));
        assert!(!topic_matches("#", "$SYS/broker"));
        assert!(topic_matches("$SYS/#", "$SYS/broker/uptime"));
        assert!(topic_matches("a/b", "a/b"));
        assert!(!topic_matches("a/b", "a/b/c"));
    }
}
