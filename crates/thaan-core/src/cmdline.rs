//! Splitting a command line into arguments, for line-based query languages (Redis, MQTT).
//!
//! Rules: whitespace separates arguments; `"…"` takes `\n`, `\r`, `\t`, `\\`, `\"` and
//! `\xHH` escapes; `'…'` is literal except `\'`. Quotes may sit inside a word
//! (`a"b c"` is one argument `ab c`).

/// Splits `line` into arguments as bytes (a `\xHH` escape can make non-UTF-8 values).
pub fn split_args(line: &str) -> Result<Vec<Vec<u8>>, String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let mut arg = Vec::new();
        while i < b.len() && !b[i].is_ascii_whitespace() {
            match b[i] {
                b'"' => {
                    i += 1;
                    loop {
                        match b.get(i) {
                            None => return Err("unclosed \" quote".into()),
                            Some(b'"') => {
                                i += 1;
                                break;
                            }
                            Some(b'\\') => {
                                let e = *b.get(i + 1).ok_or("unclosed \" quote")?;
                                i += 2;
                                match e {
                                    b'n' => arg.push(b'\n'),
                                    b'r' => arg.push(b'\r'),
                                    b't' => arg.push(b'\t'),
                                    b'x' => {
                                        let h = line
                                            .get(i..i + 2)
                                            .and_then(|h| u8::from_str_radix(h, 16).ok())
                                            .ok_or("bad \\x escape")?;
                                        arg.push(h);
                                        i += 2;
                                    }
                                    other => arg.push(other),
                                }
                            }
                            Some(&c) => {
                                arg.push(c);
                                i += 1;
                            }
                        }
                    }
                }
                b'\'' => {
                    i += 1;
                    loop {
                        match b.get(i) {
                            None => return Err("unclosed ' quote".into()),
                            Some(b'\'') => {
                                i += 1;
                                break;
                            }
                            Some(b'\\') if b.get(i + 1) == Some(&b'\'') => {
                                arg.push(b'\'');
                                i += 2;
                            }
                            Some(&c) => {
                                arg.push(c);
                                i += 1;
                            }
                        }
                    }
                }
                c => {
                    arg.push(c);
                    i += 1;
                }
            }
        }
        out.push(arg);
    }
    Ok(out)
}

/// Quotes one argument so [`split_args`] reads it back unchanged. Plain words stay bare.
pub fn quote_arg(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|c| !c.is_ascii_whitespace() && !matches!(c, b'"' | b'\'' | b'\\'));
    if plain {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(l: &str) -> Vec<String> {
        split_args(l)
            .unwrap()
            .into_iter()
            .map(|a| String::from_utf8(a).unwrap())
            .collect()
    }

    #[test]
    fn splits() {
        assert_eq!(strs("  SET  a   1 "), ["SET", "a", "1"]);
        assert_eq!(
            strs(r#"SET "user 1" 'it\'s' a"b c"d"#),
            ["SET", "user 1", "it's", "ab cd"]
        );
        assert_eq!(strs(r#"SET k "a\n\"b\x41""#), ["SET", "k", "a\n\"bA"]);
        assert!(split_args("SET \"open").is_err());
        assert_eq!(split_args(r#""\xff""#).unwrap(), vec![vec![0xff]]);
    }

    #[test]
    fn quoting_roundtrips() {
        for s in [
            "plain",
            "with space",
            "q\"uote",
            "back\\slash",
            "",
            "line\nbreak",
        ] {
            assert_eq!(strs(&format!("X {}", quote_arg(s)))[1], s);
        }
        assert_eq!(quote_arg("user:1"), "user:1");
    }
}
