//! Splits a SQL buffer into statements. Respects string literals, quoted identifiers,
//! comments, PostgreSQL dollar quoting and the MySQL-client `DELIMITER` command.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitOptions {
    /// `$tag$ … $tag$` bodies (PostgreSQL).
    pub dollar_quotes: bool,
    /// `\'` escapes inside strings (MySQL).
    pub backslash_escapes: bool,
    /// `# comment` (MySQL).
    pub hash_comments: bool,
    /// `DELIMITER //` lines change the terminator (MySQL client convention).
    pub delimiter_command: bool,
    /// `[identifier]` (SQL Server, SQLite).
    pub bracket_identifiers: bool,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            dollar_quotes: true,
            backslash_escapes: false,
            hash_comments: false,
            delimiter_command: true,
            bracket_identifiers: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    /// Statement text without the terminator, trimmed.
    pub sql: String,
    /// Byte range of `sql` in the input buffer.
    pub start: usize,
    pub end: usize,
}

pub fn split(input: &str, opts: SplitOptions) -> Vec<Statement> {
    let b = input.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut delimiter: String = ";".into();
    let mut stmt_start = 0usize;
    let mut has_code = false;
    let mut i = 0usize;
    let mut at_line_start = true;

    let push = |out: &mut Vec<Statement>, from: usize, to: usize, has_code: bool| {
        if !has_code {
            return;
        }
        let raw = &input[from..to];
        let lead = raw.len() - raw.trim_start().len();
        let trimmed = raw.trim();
        let start = from + lead;
        out.push(Statement {
            sql: trimmed.to_string(),
            start,
            end: start + trimmed.len(),
        });
    };

    while i < n {
        let c = b[i];

        // DELIMITER command: whole line, only at line start (ignoring spaces).
        if opts.delimiter_command && at_line_start && !has_code {
            let line_end = input[i..].find('\n').map(|p| i + p).unwrap_or(n);
            let line = input[i..line_end].trim();
            if line.len() > 10
                && line
                    .get(..10)
                    .is_some_and(|p| p.eq_ignore_ascii_case("delimiter "))
            {
                delimiter = line[10..].trim().to_string();
                i = line_end;
                stmt_start = i;
                continue;
            }
        }

        if c == b'\n' {
            at_line_start = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        at_line_start = false;

        // Terminator.
        if input[i..].starts_with(delimiter.as_str()) {
            push(&mut out, stmt_start, i, has_code);
            i += delimiter.len();
            stmt_start = i;
            has_code = false;
            continue;
        }

        match c {
            b'-' if i + 1 < n && b[i + 1] == b'-' => {
                i = input[i..].find('\n').map(|p| i + p).unwrap_or(n);
            }
            b'#' if opts.hash_comments => {
                i = input[i..].find('\n').map(|p| i + p).unwrap_or(n);
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i = input[i + 2..]
                    .find("*/")
                    .map(|p| i + 2 + p + 2)
                    .unwrap_or(n);
            }
            b'\'' | b'"' | b'`' => {
                has_code = true;
                i = skip_quoted(b, i, c, opts.backslash_escapes && c != b'`');
            }
            b'[' if opts.bracket_identifiers => {
                has_code = true;
                i = input[i..].find(']').map(|p| i + p + 1).unwrap_or(n);
            }
            b'$' if opts.dollar_quotes && (i == 0 || !is_ident_byte(b[i - 1])) => {
                has_code = true;
                match dollar_tag(input, i) {
                    Some(tag) => {
                        let body = i + tag.len();
                        i = input[body..]
                            .find(tag)
                            .map(|p| body + p + tag.len())
                            .unwrap_or(n);
                    }
                    None => i += 1,
                }
            }
            _ => {
                has_code = true;
                // Advance one UTF-8 char.
                i += utf8_len(c);
            }
        }
    }
    push(&mut out, stmt_start, n, has_code);
    out
}

/// The statement containing `cursor`, or the closest one before it.
pub fn statement_at(stmts: &[Statement], cursor: usize) -> Option<&Statement> {
    stmts
        .iter()
        .find(|s| cursor >= s.start && cursor <= s.end)
        .or_else(|| stmts.iter().rev().find(|s| s.end <= cursor))
        .or_else(|| stmts.first())
}

fn skip_quoted(b: &[u8], start: usize, q: u8, backslash: bool) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        if backslash && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if i + 1 < b.len() && b[i + 1] == q {
                i += 2; // doubled quote escape
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

fn dollar_tag(input: &str, i: usize) -> Option<&str> {
    let rest = &input[i + 1..];
    let end = rest.find('$')?;
    let tag = &rest[..end];
    let valid = tag.is_empty()
        || (tag
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && tag.chars().all(|c| c.is_alphanumeric() || c == '_'));
    valid.then(|| &input[i..i + end + 2])
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqls(s: &str) -> Vec<String> {
        split(s, SplitOptions::default())
            .into_iter()
            .map(|s| s.sql)
            .collect()
    }

    #[test]
    fn basic() {
        assert_eq!(sqls("select 1; select 2;"), ["select 1", "select 2"]);
        assert_eq!(sqls("select 1"), ["select 1"]);
        assert_eq!(sqls("  ;; \n"), Vec::<String>::new());
    }

    #[test]
    fn quotes_and_comments() {
        assert_eq!(
            sqls("select ';' as a; -- c;\nselect \"x;y\" /* ; */ from t;"),
            ["select ';' as a", "-- c;\nselect \"x;y\" /* ; */ from t"]
        );
        assert_eq!(sqls("select 'it''s; ok'"), ["select 'it''s; ok'"]);
        assert_eq!(sqls("-- only a comment;\n"), Vec::<String>::new());
    }

    #[test]
    fn dollar_quoting() {
        let s = "create function f() returns int as $$ begin return 1; end $$ language plpgsql; select $1";
        assert_eq!(
            sqls(s),
            [
                "create function f() returns int as $$ begin return 1; end $$ language plpgsql",
                "select $1"
            ]
        );
        assert_eq!(
            sqls("select $tag$ a;b $tag$; select 2"),
            ["select $tag$ a;b $tag$", "select 2"]
        );
    }

    #[test]
    fn delimiter_command() {
        let s = "DELIMITER //\ncreate procedure p() begin select 1; end//\nDELIMITER ;\nselect 2;";
        assert_eq!(
            sqls(s),
            ["create procedure p() begin select 1; end", "select 2"]
        );
    }

    #[test]
    fn mysql_backslash() {
        let o = SplitOptions {
            backslash_escapes: true,
            hash_comments: true,
            ..Default::default()
        };
        let v: Vec<_> = split("select 'a\\';b'; # x;\nselect 2", o)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(v, ["select 'a\\';b'", "# x;\nselect 2"]);
    }

    #[test]
    fn ranges_and_cursor() {
        let s = "select 1;\n\nselect 2;";
        let st = split(s, SplitOptions::default());
        assert_eq!(&s[st[1].start..st[1].end], "select 2");
        assert_eq!(statement_at(&st, 3).unwrap().sql, "select 1");
        assert_eq!(statement_at(&st, 10).unwrap().sql, "select 1");
        assert_eq!(statement_at(&st, 14).unwrap().sql, "select 2");
    }

    #[test]
    fn unicode() {
        assert_eq!(
            sqls("select 'ไทย'; select 'é'"),
            ["select 'ไทย'", "select 'é'"]
        );
    }
}
