//! RESP2, the Redis wire protocol, written from the protocol spec at
//! <https://redis.io/docs/latest/develop/reference/protocol-spec/>.

use std::io::{self, BufRead, Write};

/// One reply from the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// `+OK`
    Status(String),
    /// `-ERR …`
    Error(String),
    Int(i64),
    /// `$-1` or `*-1`
    Nil,
    Bulk(Vec<u8>),
    Array(Vec<Reply>),
}

impl Reply {
    /// Bytes of a status or bulk reply, or the integer as text.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        match self {
            Reply::Status(s) => Some(s.as_bytes().to_vec()),
            Reply::Bulk(b) => Some(b.clone()),
            Reply::Int(i) => Some(i.to_string().into_bytes()),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<String> {
        self.bytes()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Reply::Int(i) => Some(*i),
            Reply::Bulk(b) => std::str::from_utf8(b).ok()?.trim().parse().ok(),
            Reply::Status(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn array(self) -> Vec<Reply> {
        match self {
            Reply::Array(a) => a,
            Reply::Nil => Vec::new(),
            other => vec![other],
        }
    }
}

/// Writes one command as an array of bulk strings.
pub fn write_command<W: Write>(w: &mut W, args: &[&[u8]]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(16 + args.iter().map(|a| a.len() + 16).sum::<usize>());
    buf.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for a in args {
        buf.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
        buf.extend_from_slice(a);
        buf.extend_from_slice(b"\r\n");
    }
    w.write_all(&buf)?;
    w.flush()
}

/// Bulk strings above this size are refused, so a broken or hostile server cannot make
/// us allocate without bound.
const MAX_BULK: usize = 512 * 1024 * 1024;
/// Arrays above this many elements are refused (same reason).
const MAX_ARRAY: usize = 64 * 1024 * 1024;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn read_line<R: BufRead>(r: &mut R) -> io::Result<String> {
    let mut line = Vec::new();
    r.read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed by server",
        ));
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| bad("reply line is not UTF-8"))
}

pub fn read_reply<R: BufRead>(r: &mut R) -> io::Result<Reply> {
    let line = read_line(r)?;
    let (kind, rest) = line.split_at(line.len().min(1));
    let num = || -> io::Result<i64> { rest.parse().map_err(|_| bad("bad length in reply")) };
    Ok(match kind {
        "+" => Reply::Status(rest.to_string()),
        "-" => Reply::Error(rest.to_string()),
        ":" => Reply::Int(num()?),
        "$" => {
            let n = num()?;
            if n < 0 {
                return Ok(Reply::Nil);
            }
            let n = n as usize;
            if n > MAX_BULK {
                return Err(bad("bulk reply too large"));
            }
            let mut buf = vec![0; n + 2];
            r.read_exact(&mut buf)?;
            buf.truncate(n);
            Reply::Bulk(buf)
        }
        "*" => {
            let n = num()?;
            if n < 0 {
                return Ok(Reply::Nil);
            }
            let n = n as usize;
            if n > MAX_ARRAY {
                return Err(bad("array reply too large"));
            }
            let mut items = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                items.push(read_reply(r)?);
            }
            Reply::Array(items)
        }
        _ => return Err(bad("unknown reply type")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_commands() {
        let mut out = Vec::new();
        write_command(&mut out, &[b"SET", b"k", b"a b"]).unwrap();
        assert_eq!(out, b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$3\r\na b\r\n");
    }

    #[test]
    fn decodes_replies() {
        let data = b"*4\r\n+OK\r\n:42\r\n$-1\r\n*2\r\n$3\r\nfoo\r\n-ERR bad\r\n";
        let mut r = &data[..];
        assert_eq!(
            read_reply(&mut r).unwrap(),
            Reply::Array(vec![
                Reply::Status("OK".into()),
                Reply::Int(42),
                Reply::Nil,
                Reply::Array(vec![
                    Reply::Bulk(b"foo".to_vec()),
                    Reply::Error("ERR bad".into())
                ]),
            ])
        );
        let mut r = &b"$5\r\na\r\nb\r\r\n"[..];
        assert_eq!(
            read_reply(&mut r).unwrap(),
            Reply::Bulk(b"a\r\nb\r".to_vec())
        );
        assert!(read_reply(&mut &b""[..]).is_err());
    }
}
