//! Decoding of PostgreSQL binary-format values ("send" functions of each type), from
//! the type documentation. Unknown types fall back to UTF-8 text or hex.

use thaan_core::Value;
use postgres::types::{Kind, Type};

/// Days from 1970-01-01 to 2000-01-01 (the PostgreSQL epoch).
const PG_EPOCH_DAYS: i64 = 10_957;

pub fn value(ty: &Type, raw: &[u8]) -> Value {
    match ty.kind() {
        Kind::Array(elem) => return array(elem, raw).map(Value::Text).unwrap_or_else(|| fallback(raw)),
        Kind::Domain(base) => return value(base, raw),
        Kind::Enum(_) => return Value::Text(String::from_utf8_lossy(raw).into_owned()),
        _ => {}
    }
    match *ty {
        Type::BOOL => Value::Bool(raw.first().is_some_and(|b| *b != 0)),
        Type::INT2 => be::<2>(raw).map(|b| Value::Int(i16::from_be_bytes(b) as i64)).unwrap_or_else(|| fallback(raw)),
        Type::INT4 => be::<4>(raw).map(|b| Value::Int(i32::from_be_bytes(b) as i64)).unwrap_or_else(|| fallback(raw)),
        Type::INT8 => be::<8>(raw).map(|b| Value::Int(i64::from_be_bytes(b))).unwrap_or_else(|| fallback(raw)),
        Type::OID | Type::XID | Type::CID | Type::REGCLASS | Type::REGPROC | Type::REGTYPE => {
            be::<4>(raw).map(|b| Value::Int(u32::from_be_bytes(b) as i64)).unwrap_or_else(|| fallback(raw))
        }
        Type::FLOAT4 => be::<4>(raw).map(|b| Value::Float(f32::from_be_bytes(b) as f64)).unwrap_or_else(|| fallback(raw)),
        Type::FLOAT8 => be::<8>(raw).map(|b| Value::Float(f64::from_be_bytes(b))).unwrap_or_else(|| fallback(raw)),
        Type::NUMERIC => numeric(raw).map(Value::Text).unwrap_or_else(|| fallback(raw)),
        Type::BYTEA => Value::Bytes(raw.to_vec()),
        Type::UUID if raw.len() == 16 => Value::Text(uuid(raw)),
        Type::JSONB if raw.first() == Some(&1) => Value::Text(String::from_utf8_lossy(&raw[1..]).into_owned()),
        Type::DATE => be::<4>(raw).map(|b| Value::Text(date(i32::from_be_bytes(b)))).unwrap_or_else(|| fallback(raw)),
        Type::TIMESTAMP => be::<8>(raw).map(|b| Value::Text(timestamp(i64::from_be_bytes(b), false))).unwrap_or_else(|| fallback(raw)),
        Type::TIMESTAMPTZ => be::<8>(raw).map(|b| Value::Text(timestamp(i64::from_be_bytes(b), true))).unwrap_or_else(|| fallback(raw)),
        Type::TIME => be::<8>(raw).map(|b| Value::Text(time_of_day(i64::from_be_bytes(b)))).unwrap_or_else(|| fallback(raw)),
        Type::INTERVAL if raw.len() == 16 => {
            let us = i64::from_be_bytes(raw[0..8].try_into().unwrap_or_default());
            let days = i32::from_be_bytes(raw[8..12].try_into().unwrap_or_default());
            let months = i32::from_be_bytes(raw[12..16].try_into().unwrap_or_default());
            Value::Text(interval(months, days, us))
        }
        Type::INET | Type::CIDR => inet(raw).map(Value::Text).unwrap_or_else(|| fallback(raw)),
        _ => fallback(raw),
    }
}

fn be<const N: usize>(raw: &[u8]) -> Option<[u8; N]> {
    raw.try_into().ok()
}

fn fallback(raw: &[u8]) -> Value {
    match std::str::from_utf8(raw) {
        Ok(s) => Value::Text(s.to_string()),
        Err(_) => Value::Bytes(raw.to_vec()),
    }
}

/// Text of an array value, `{a,b,NULL}` style (one or more dimensions).
fn array(elem: &Type, raw: &[u8]) -> Option<String> {
    let rd_i32 = |p: usize| raw.get(p..p + 4).map(|b| i32::from_be_bytes(b.try_into().unwrap_or_default()));
    let ndim = rd_i32(0)? as usize;
    if ndim == 0 {
        return Some("{}".into());
    }
    let mut dims = Vec::with_capacity(ndim);
    for d in 0..ndim {
        dims.push(rd_i32(12 + d * 8)?.max(0) as usize);
    }
    let mut pos = 12 + ndim * 8;
    let total: usize = dims.iter().product();
    let mut items = Vec::with_capacity(total);
    for _ in 0..total {
        let len = rd_i32(pos)?;
        pos += 4;
        if len < 0 {
            items.push("NULL".to_string());
            continue;
        }
        let bytes = raw.get(pos..pos + len as usize)?;
        pos += len as usize;
        let s = match value(elem, bytes) {
            Value::Text(s) => {
                let needs = s.is_empty() || s.eq_ignore_ascii_case("null") || s.contains(|c: char| ",{}\"\\ ".contains(c));
                if needs { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) } else { s }
            }
            other => other.to_string(),
        };
        items.push(s);
    }
    fn nest(items: &[String], dims: &[usize]) -> String {
        if dims.len() == 1 {
            return format!("{{{}}}", items.join(","));
        }
        let chunk = items.len() / dims[0].max(1);
        let parts: Vec<String> = items.chunks(chunk.max(1)).map(|c| nest(c, &dims[1..])).collect();
        format!("{{{}}}", parts.join(","))
    }
    Some(nest(&items, &dims))
}

/// NUMERIC: ndigits, weight, sign, dscale, then base-10000 digits.
pub fn numeric(raw: &[u8]) -> Option<String> {
    let u16_at = |p: usize| raw.get(p..p + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let ndigits = u16_at(0)? as usize;
    let weight = u16_at(2)? as i16 as i64;
    let sign = u16_at(4)?;
    let dscale = u16_at(6)? as usize;
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let mut digits = Vec::with_capacity(ndigits);
    for i in 0..ndigits {
        digits.push(u16_at(8 + i * 2)?);
    }
    let digit = |k: i64| if k >= 0 && (k as usize) < digits.len() { digits[k as usize] } else { 0 };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    } else {
        for k in 0..=weight {
            if k == 0 {
                s.push_str(&digit(k).to_string());
            } else {
                s.push_str(&format!("{:04}", digit(k)));
            }
        }
    }
    if dscale > 0 {
        let mut frac = String::new();
        let mut k = weight + 1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", digit(k)));
            k += 1;
        }
        frac.truncate(dscale);
        s.push('.');
        s.push_str(&frac);
    }
    if s == "-0" || s.starts_with("-0.") && s[3..].chars().all(|c| c == '0') {
        s.remove(0);
    }
    Some(s)
}

fn uuid(b: &[u8]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Civil date from days since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn ymd(days_since_pg_epoch: i64) -> String {
    let (y, m, d) = civil(days_since_pg_epoch + PG_EPOCH_DAYS);
    if y <= 0 {
        format!("{:04}-{m:02}-{d:02} BC", 1 - y)
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

pub fn date(days: i32) -> String {
    match days {
        i32::MAX => "infinity".into(),
        i32::MIN => "-infinity".into(),
        d => ymd(d as i64),
    }
}

pub fn time_of_day(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let frac = us.rem_euclid(1_000_000);
    let mut s = format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60);
    if frac > 0 {
        s.push_str(format!(".{frac:06}").trim_end_matches('0'));
    }
    s
}

/// TIMESTAMPTZ values are shown in UTC.
pub fn timestamp(us: i64, tz: bool) -> String {
    match us {
        i64::MAX => return "infinity".into(),
        i64::MIN => return "-infinity".into(),
        _ => {}
    }
    let day = us.div_euclid(86_400_000_000);
    let rest = us.rem_euclid(86_400_000_000);
    let mut s = format!("{} {}", ymd(day), time_of_day(rest));
    if tz {
        s.push_str("+00");
    }
    s
}

pub fn interval(months: i32, days: i32, us: i64) -> String {
    let mut parts = Vec::new();
    let (y, mo) = (months / 12, months % 12);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n.abs() == 1 { "" } else { "s" });
    if y != 0 {
        parts.push(plural(y as i64, "year"));
    }
    if mo != 0 {
        parts.push(plural(mo as i64, "mon"));
    }
    if days != 0 {
        parts.push(plural(days as i64, "day"));
    }
    if us != 0 || parts.is_empty() {
        let sign = if us < 0 { "-" } else { "" };
        parts.push(format!("{sign}{}", time_of_day(us.abs())));
    }
    parts.join(" ")
}

fn inet(raw: &[u8]) -> Option<String> {
    let family = *raw.first()?;
    let bits = *raw.get(1)?;
    let len = *raw.get(3)? as usize;
    let addr = raw.get(4..4 + len)?;
    let text = match family {
        2 if len == 4 => format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]),
        3 if len == 16 => {
            let segs: Vec<u16> = addr.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            std::net::Ipv6Addr::new(segs[0], segs[1], segs[2], segs[3], segs[4], segs[5], segs[6], segs[7]).to_string()
        }
        _ => return None,
    };
    let full = if family == 2 { 32 } else { 128 };
    Some(if bits as u32 == full { text } else { format!("{text}/{bits}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(ndigits: &[u16], weight: i16, sign: u16, dscale: u16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(ndigits.len() as u16).to_be_bytes());
        v.extend_from_slice(&weight.to_be_bytes());
        v.extend_from_slice(&sign.to_be_bytes());
        v.extend_from_slice(&dscale.to_be_bytes());
        for d in ndigits {
            v.extend_from_slice(&d.to_be_bytes());
        }
        v
    }

    #[test]
    fn numerics() {
        // 1234.5 with scale 3 → digits [1234, 5000], weight 0
        assert_eq!(numeric(&num(&[1234, 5000], 0, 0x4000, 3)).unwrap(), "-1234.500");
        // 0.001 → digits [10], weight -1
        assert_eq!(numeric(&num(&[10], -1, 0, 3)).unwrap(), "0.001");
        // 12345678 → [1234, 5678], weight 1
        assert_eq!(numeric(&num(&[1234, 5678], 1, 0, 0)).unwrap(), "12345678");
        // 10000 → [1], weight 1 (trailing zero groups omitted)
        assert_eq!(numeric(&num(&[1], 1, 0, 0)).unwrap(), "10000");
        assert_eq!(numeric(&num(&[], 0, 0, 2)).unwrap(), "0.00");
        assert_eq!(numeric(&num(&[], 0, 0xC000, 0)).unwrap(), "NaN");
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(date(0), "2000-01-01");
        assert_eq!(date(-1), "1999-12-31");
        assert_eq!(date(8825), "2024-02-29");
        assert_eq!(timestamp(0, true), "2000-01-01 00:00:00+00");
        assert_eq!(timestamp(-1, false), "1999-12-31 23:59:59.999999");
        assert_eq!(time_of_day(49_501_250_000), "13:45:01.25");
        assert_eq!(interval(14, 3, 3_600_000_000), "1 year 2 mons 3 days 01:00:00");
        assert_eq!(interval(0, 0, 0), "00:00:00");
    }

    #[test]
    fn misc() {
        assert_eq!(
            uuid(&[0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x6d, 0x6b, 0xb9, 0xbd, 0x38, 0x0a, 0x11]),
            "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"
        );
        assert_eq!(inet(&[2, 24, 1, 4, 10, 0, 0, 0]).unwrap(), "10.0.0.0/24");
        assert_eq!(inet(&[2, 32, 0, 4, 127, 0, 0, 1]).unwrap(), "127.0.0.1");
    }
}
