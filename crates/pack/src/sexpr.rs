//! Minimal S-expression parser for `.kicad_sym` files.
//!
//! Atoms are barewords, `Str`s are double-quoted (with `\"` and `\\` escapes),
//! lists are paren-delimited. Whitespace (including newlines) separates tokens.
//! Comments aren't used in the KiCad library — none seen across 22,756 files,
//! so we don't bother parsing them.

#[derive(Debug, Clone)]
pub enum Sexpr {
    Atom(String),
    Str(String),
    List(Vec<Sexpr>),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected end of input")]
    Eof,
    #[error("expected '(' at byte {0}")]
    ExpectedOpen(usize),
    #[error("unbalanced ')' at byte {0}")]
    UnbalancedClose(usize),
    #[error("unterminated string starting at byte {0}")]
    UnterminatedString(usize),
    #[error("trailing garbage after root expression at byte {0}")]
    Trailing(usize),
}

pub fn parse(src: &str) -> Result<Sexpr, ParseError> {
    let mut cur = 0;
    skip_ws(src.as_bytes(), &mut cur);
    let root = parse_one(src.as_bytes(), &mut cur)?;
    skip_ws(src.as_bytes(), &mut cur);
    if cur != src.len() {
        return Err(ParseError::Trailing(cur));
    }
    Ok(root)
}

fn parse_one(b: &[u8], cur: &mut usize) -> Result<Sexpr, ParseError> {
    if *cur >= b.len() {
        return Err(ParseError::Eof);
    }
    match b[*cur] {
        b'(' => parse_list(b, cur),
        b'"' => parse_str(b, cur).map(Sexpr::Str),
        b')' => Err(ParseError::UnbalancedClose(*cur)),
        _ => Ok(Sexpr::Atom(parse_atom(b, cur))),
    }
}

fn parse_list(b: &[u8], cur: &mut usize) -> Result<Sexpr, ParseError> {
    if *cur >= b.len() || b[*cur] != b'(' {
        return Err(ParseError::ExpectedOpen(*cur));
    }
    *cur += 1;
    let mut items = Vec::new();
    loop {
        skip_ws(b, cur);
        if *cur >= b.len() {
            return Err(ParseError::Eof);
        }
        if b[*cur] == b')' {
            *cur += 1;
            return Ok(Sexpr::List(items));
        }
        items.push(parse_one(b, cur)?);
    }
}

fn parse_str(b: &[u8], cur: &mut usize) -> Result<String, ParseError> {
    let start = *cur;
    debug_assert_eq!(b[*cur], b'"');
    *cur += 1;
    let mut out = String::new();
    while *cur < b.len() {
        let c = b[*cur];
        if c == b'\\' {
            if *cur + 1 >= b.len() {
                return Err(ParseError::UnterminatedString(start));
            }
            // KiCad escapes: \\ \" \n \r \t — anything else passes through
            let esc = b[*cur + 1];
            let pushed = match esc {
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'"' => '"',
                b'\\' => '\\',
                other => other as char,
            };
            out.push(pushed);
            *cur += 2;
        } else if c == b'"' {
            *cur += 1;
            return Ok(out);
        } else {
            out.push(c as char);
            *cur += 1;
        }
    }
    Err(ParseError::UnterminatedString(start))
}

fn parse_atom(b: &[u8], cur: &mut usize) -> String {
    let start = *cur;
    while *cur < b.len() {
        let c = b[*cur];
        if c.is_ascii_whitespace() || c == b'(' || c == b')' {
            break;
        }
        *cur += 1;
    }
    // UTF-8 safety: atoms in .kicad_sym are ASCII identifiers / numbers
    String::from_utf8_lossy(&b[start..*cur]).into_owned()
}

fn skip_ws(b: &[u8], cur: &mut usize) {
    while *cur < b.len() && b[*cur].is_ascii_whitespace() {
        *cur += 1;
    }
}

// ---------- helpers for downstream consumers ----------

impl Sexpr {
    /// If this is a `(head ...)` list, return `Some(("head", &rest))`.
    pub fn list_head(&self) -> Option<(&str, &[Sexpr])> {
        match self {
            Sexpr::List(items) => match items.first()? {
                Sexpr::Atom(h) => Some((h.as_str(), &items[1..])),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_atom(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_str_node(&self) -> Option<&str> {
        match self {
            Sexpr::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Either an atom or a string — the format uses both interchangeably for
    /// some keywords (e.g. property names are strings, but flag values like
    /// `yes`/`no` are atoms).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(a) => Some(a),
            Sexpr::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Sexpr]> {
        match self {
            Sexpr::List(items) => Some(items),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple_list() {
        let s = parse("(a (b c) \"hello world\")").unwrap();
        let items = s.as_list().unwrap();
        assert_eq!(items[0].as_atom(), Some("a"));
        let (h, rest) = items[1].list_head().unwrap();
        assert_eq!(h, "b");
        assert_eq!(rest[0].as_atom(), Some("c"));
        assert_eq!(items[2].as_str_node(), Some("hello world"));
    }

    #[test]
    fn escapes_in_strings() {
        let s = parse(r#"(p "with \"quotes\" and \\slash")"#).unwrap();
        let v = s.as_list().unwrap()[1].as_str_node().unwrap();
        assert_eq!(v, "with \"quotes\" and \\slash");
    }

    #[test]
    fn fails_on_unbalanced_close() {
        assert!(parse("(a))").is_err());
    }

    #[test]
    fn fails_on_unterminated_string() {
        assert!(parse(r#"("oh no"#).is_err());
    }
}
