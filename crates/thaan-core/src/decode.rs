//! Decoders for stored values: what a key-value store often keeps instead of plain
//! text. Compressed bytes (gzip, zlib, raw deflate) and base64 decode to bytes;
//! MessagePack, Python pickle and Protobuf (without a schema) decode to indented JSON.
//!
//! Written from the formats' public specs: RFC 1950–1952 and 4648, the MessagePack spec
//! (github.com/msgpack/msgpack), and the Protobuf wire-format docs. Pickle goes through
//! the `serde-pickle` crate. Output is capped at [`MAX_OUT`] so a small hostile input
//! cannot expand without bound.

use serde_json::{Map, Value as Json};

/// Most bytes a decoder may produce.
pub const MAX_OUT: usize = 16 * 1024 * 1024;
/// Deepest nesting followed in MessagePack and Protobuf.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoder {
    None,
    Gzip,
    Zlib,
    Deflate,
    Base64,
    Msgpack,
    Pickle,
    Protobuf,
}

impl Decoder {
    pub const ALL: [Decoder; 8] = [
        Decoder::None,
        Decoder::Gzip,
        Decoder::Zlib,
        Decoder::Deflate,
        Decoder::Base64,
        Decoder::Msgpack,
        Decoder::Pickle,
        Decoder::Protobuf,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Decoder::None => "None",
            Decoder::Gzip => "GZip",
            Decoder::Zlib => "Zlib",
            Decoder::Deflate => "Deflate",
            Decoder::Base64 => "Base64",
            Decoder::Msgpack => "MessagePack",
            Decoder::Pickle => "Pickle",
            Decoder::Protobuf => "Protobuf",
        }
    }

    /// Whether this build can run the decoder (compression and pickle need the
    /// `decoders` feature).
    pub fn available(self) -> bool {
        match self {
            Decoder::Gzip | Decoder::Zlib | Decoder::Deflate | Decoder::Pickle => {
                cfg!(feature = "decoders")
            }
            _ => true,
        }
    }
}

/// Decodes `input`; structured formats come back as indented JSON text.
pub fn decode(d: Decoder, input: &[u8]) -> Result<Vec<u8>, String> {
    match d {
        Decoder::None => Ok(input.to_vec()),
        Decoder::Base64 => base64_decode(input),
        Decoder::Msgpack => {
            let mut r = Reader { b: input, at: 0 };
            let v = msgpack(&mut r, 0)?;
            if r.at != input.len() {
                return Err(format!(
                    "not MessagePack: {} bytes left after the value",
                    input.len() - r.at
                ));
            }
            pretty(&v)
        }
        Decoder::Protobuf => {
            let v = protobuf(input, 0).ok_or("not a Protobuf message")?;
            pretty(&v)
        }
        #[cfg(feature = "decoders")]
        Decoder::Gzip => inflate(flate2::read::GzDecoder::new(input)),
        #[cfg(feature = "decoders")]
        Decoder::Zlib => inflate(flate2::read::ZlibDecoder::new(input)),
        #[cfg(feature = "decoders")]
        Decoder::Deflate => inflate(flate2::read::DeflateDecoder::new(input)),
        #[cfg(feature = "decoders")]
        Decoder::Pickle => {
            let opts = serde_pickle::DeOptions::new()
                .replace_unresolved_globals()
                .decode_strings();
            let v = serde_pickle::value_from_slice(input, opts)
                .map_err(|e| format!("not a pickle: {e}"))?;
            pretty(&pickle_json(v))
        }
        #[cfg(not(feature = "decoders"))]
        Decoder::Gzip | Decoder::Zlib | Decoder::Deflate | Decoder::Pickle => {
            Err(format!("{} is not in this build", d.label()))
        }
    }
}

fn pretty(v: &Json) -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(v).map_err(|e| e.to_string())
}

#[cfg(feature = "decoders")]
fn inflate(r: impl std::io::Read) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut out = Vec::new();
    r.take(MAX_OUT as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("cannot decompress: {e}"))?;
    if out.len() > MAX_OUT {
        return Err(format!(
            "decompresses to more than {} MB",
            MAX_OUT / 1024 / 1024
        ));
    }
    Ok(out)
}

#[cfg(feature = "decoders")]
fn pickle_json(v: serde_pickle::Value) -> Json {
    use serde_pickle::Value as P;
    match v {
        P::None => Json::Null,
        P::Bool(b) => b.into(),
        P::I64(i) => i.into(),
        P::Int(i) => Json::String(i.to_string()),
        P::F64(f) => serde_json::Number::from_f64(f).map_or(Json::Null, Json::Number),
        P::Bytes(b) => Json::String(bytes_text(&b)),
        P::String(s) => s.into(),
        P::List(l) | P::Tuple(l) => Json::Array(l.into_iter().map(pickle_json).collect()),
        P::Set(s) | P::FrozenSet(s) => {
            Json::Array(s.into_iter().map(|h| pickle_json(h.into_value())).collect())
        }
        P::Dict(d) => Json::Object(
            d.into_iter()
                .map(|(k, v)| {
                    let key = match pickle_json(k.into_value()) {
                        Json::String(s) => s,
                        other => other.to_string(),
                    };
                    (key, pickle_json(v))
                })
                .collect(),
        ),
    }
}

/// Bytes inside a JSON view: text when UTF-8, else `0x…` hex.
fn bytes_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let mut s = String::from("0x");
            for x in b {
                s.push_str(&format!("{x:02x}"));
            }
            s
        }
    }
}

/// Base64, standard or URL-safe alphabet, padding optional, whitespace ignored.
pub fn base64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let chars: Vec<u8> = input
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let body = chars
        .strip_suffix(b"==")
        .or_else(|| chars.strip_suffix(b"="))
        .unwrap_or(&chars);
    if body.len() % 4 == 1 {
        return Err("not base64: wrong length".into());
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for chunk in body.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = val(*c).ok_or_else(|| format!("not base64: `{}`", *c as char))?;
            n |= v << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len());
        let end = end.ok_or("not MessagePack: ends early")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn uint(&mut self, n: usize) -> Result<u64, String> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |a, b| (a << 8) | u64::from(*b)))
    }
}

fn msgpack(r: &mut Reader, depth: usize) -> Result<Json, String> {
    if depth > MAX_DEPTH {
        return Err("not MessagePack: nested too deep".into());
    }
    let t = r.u8()?;
    let int = |i: i64| Json::from(i);
    Ok(match t {
        0x00..=0x7f => int(i64::from(t)),
        0x80..=0x8f => msgpack_map(r, usize::from(t & 0x0f), depth)?,
        0x90..=0x9f => msgpack_arr(r, usize::from(t & 0x0f), depth)?,
        0xa0..=0xbf => Json::String(String::from_utf8_lossy(r.take(usize::from(t & 0x1f))?).into()),
        0xc0 => Json::Null,
        0xc2 => false.into(),
        0xc3 => true.into(),
        0xc4..=0xc6 => {
            let n = r.uint(1 << (t - 0xc4))? as usize;
            Json::String(bytes_text(r.take(n)?))
        }
        0xc7..=0xc9 => {
            let n = r.uint(1 << (t - 0xc7))? as usize;
            let kind = r.u8()? as i8;
            ext(kind, r.take(n)?)
        }
        0xca => Json::from(f64::from(f32::from_bits(r.uint(4)? as u32))),
        0xcb => serde_json::Number::from_f64(f64::from_bits(r.uint(8)?))
            .map_or(Json::Null, Json::Number),
        0xcc..=0xcf => Json::from(r.uint(1 << (t - 0xcc))?),
        0xd0 => int(i64::from(r.u8()? as i8)),
        0xd1 => int(i64::from(r.uint(2)? as u16 as i16)),
        0xd2 => int(i64::from(r.uint(4)? as u32 as i32)),
        0xd3 => int(r.uint(8)? as i64),
        0xd4..=0xd8 => {
            let n = 1usize << (t - 0xd4);
            let kind = r.u8()? as i8;
            ext(kind, r.take(n)?)
        }
        0xd9..=0xdb => {
            let n = r.uint(1 << (t - 0xd9))? as usize;
            Json::String(String::from_utf8_lossy(r.take(n)?).into())
        }
        0xdc | 0xdd => {
            let n = r.uint(if t == 0xdc { 2 } else { 4 })? as usize;
            msgpack_arr(r, n, depth)?
        }
        0xde | 0xdf => {
            let n = r.uint(if t == 0xde { 2 } else { 4 })? as usize;
            msgpack_map(r, n, depth)?
        }
        0xe0..=0xff => int(i64::from(t as i8)),
        0xc1 => return Err("not MessagePack: reserved byte 0xc1".into()),
    })
}

fn ext(kind: i8, data: &[u8]) -> Json {
    // Type -1 is the standard timestamp.
    if kind == -1 && matches!(data.len(), 4 | 8 | 12) {
        let secs = match data.len() {
            4 => i64::from(u32::from_be_bytes([data[0], data[1], data[2], data[3]])),
            8 => (u64::from_be_bytes(data.try_into().unwrap_or([0; 8])) & 0x3_ffff_ffff) as i64,
            _ => i64::from_be_bytes(data[4..12].try_into().unwrap_or([0; 8])),
        };
        return Json::from(format!("timestamp {secs}"));
    }
    let mut o = Map::new();
    o.insert("ext".into(), kind.into());
    o.insert("data".into(), Json::String(bytes_text(data)));
    Json::Object(o)
}

fn msgpack_arr(r: &mut Reader, n: usize, depth: usize) -> Result<Json, String> {
    // Every element takes at least one byte: refuse counts the input cannot hold.
    if n > r.b.len() - r.at {
        return Err("not MessagePack: array longer than the data".into());
    }
    (0..n)
        .map(|_| msgpack(r, depth + 1))
        .collect::<Result<Vec<_>, _>>()
        .map(Json::Array)
}

fn msgpack_map(r: &mut Reader, n: usize, depth: usize) -> Result<Json, String> {
    if n.saturating_mul(2) > r.b.len() - r.at {
        return Err("not MessagePack: map longer than the data".into());
    }
    let mut o = Map::new();
    for _ in 0..n {
        let k = match msgpack(r, depth + 1)? {
            Json::String(s) => s,
            other => other.to_string(),
        };
        o.insert(k, msgpack(r, depth + 1)?);
    }
    Ok(Json::Object(o))
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// A Protobuf message without its schema: `{"<field number>": value}`. Repeated fields
/// become arrays; a length-delimited field shows as a nested message when it parses as
/// one, else as text, else as hex. `None` when the bytes are not a whole message.
fn protobuf(b: &[u8], depth: usize) -> Option<Json> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut at = 0;
    let mut out = Map::new();
    while at < b.len() {
        let tag = varint(b, &mut at)?;
        let (field, wire) = (tag >> 3, tag & 7);
        if field == 0 {
            return None;
        }
        let v = match wire {
            0 => Json::from(varint(b, &mut at)?),
            1 => {
                let x = u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?);
                at += 8;
                Json::from(x)
            }
            2 => {
                let n = usize::try_from(varint(b, &mut at)?).ok()?;
                let s = b.get(at..at.checked_add(n)?)?;
                at += n;
                match protobuf(s, depth + 1).filter(|_| !s.is_empty()) {
                    Some(m)
                        if std::str::from_utf8(s)
                            .map_or(true, |t| t.chars().any(char::is_control)) =>
                    {
                        m
                    }
                    _ => Json::String(bytes_text(s)),
                }
            }
            5 => {
                let x = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?);
                at += 4;
                Json::from(x)
            }
            _ => return None,
        };
        let key = field.to_string();
        match out.get_mut(&key) {
            // Arrays only come from repetition: nested messages decode to objects.
            Some(Json::Array(a)) => a.push(v),
            Some(prev) => {
                let first = prev.take();
                *prev = Json::Array(vec![first, v]);
            }
            None => {
                out.insert(key, v);
            }
        }
    }
    Some(Json::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(d: Decoder, b: &[u8]) -> String {
        String::from_utf8(decode(d, b).unwrap()).unwrap()
    }

    #[test]
    fn base64_both_alphabets() {
        assert_eq!(base64_decode(b"aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode(b"aGk").unwrap(), b"hi");
        assert_eq!(base64_decode(b"-_8").unwrap(), [0xfb, 0xff]);
        assert!(base64_decode(b"a").is_err());
    }

    #[test]
    fn msgpack_to_json() {
        // {"a": 1, "b": [true, nil, "x"], "c": -1}
        let b = [
            0x83, 0xa1, b'a', 0x01, 0xa1, b'b', 0x93, 0xc3, 0xc0, 0xa1, b'x', 0xa1, b'c', 0xff,
        ];
        let v: Json = serde_json::from_slice(&decode(Decoder::Msgpack, &b).unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"a": 1, "b": [true, null, "x"], "c": -1})
        );
        assert!(decode(Decoder::Msgpack, &[0x93, 0x01]).is_err());
        assert!(decode(Decoder::Msgpack, &[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn protobuf_without_schema() {
        // field 1 varint 150, field 2 "testing", field 3 nested {1: 1}, field 1 again 2.
        let b = [
            0x08, 0x96, 0x01, 0x12, 0x07, b't', b'e', b's', b't', b'i', b'n', b'g', 0x1a, 0x02,
            0x08, 0x01, 0x08, 0x02,
        ];
        let v: Json = serde_json::from_slice(&decode(Decoder::Protobuf, &b).unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"1": [150, 2], "2": "testing", "3": {"1": 1}})
        );
        assert!(decode(Decoder::Protobuf, b"\xff\xff").is_err());
    }

    #[cfg(feature = "decoders")]
    #[test]
    fn compressed_and_pickled() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"hello gzip").unwrap();
        assert_eq!(text(Decoder::Gzip, &gz.finish().unwrap()), "hello gzip");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"zz").unwrap();
        assert_eq!(text(Decoder::Zlib, &z.finish().unwrap()), "zz");
        assert!(decode(Decoder::Gzip, b"plain").is_err());

        let mut dict = std::collections::BTreeMap::new();
        dict.insert(
            serde_pickle::HashableValue::String("n".into()),
            serde_pickle::Value::I64(3),
        );
        let bytes =
            serde_pickle::value_to_vec(&serde_pickle::Value::Dict(dict), Default::default())
                .unwrap();
        let v: Json = serde_json::from_slice(&decode(Decoder::Pickle, &bytes).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"n": 3}));
    }

    #[test]
    fn none_passes_through() {
        assert_eq!(text(Decoder::None, b"abc"), "abc");
    }
}
