use super::*;

/// Lexical highlighting shared by MySQL, PostgreSQL and SQL Server. This is not
/// a parser: dialect-dependent constructs deliberately use a permissive union.
pub fn highlight_sql(sql: &str, dark_mode: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut offset = 0;
    while offset < sql.len() {
        let (length, token) = next_token(&sql[offset..]);
        let color = match (token, dark_mode) {
            (Token::Plain, true) => Color32::from_rgb(220, 223, 228),
            (Token::Plain, false) => Color32::from_rgb(35, 39, 45),
            (Token::Keyword, true) => Color32::from_rgb(130, 180, 255),
            (Token::Keyword, false) => Color32::from_rgb(25, 75, 165),
            (Token::String, true) => Color32::from_rgb(166, 211, 135),
            (Token::String, false) => Color32::from_rgb(40, 112, 45),
            (Token::Comment, true) => Color32::from_rgb(143, 155, 166),
            (Token::Comment, false) => Color32::from_rgb(100, 108, 115),
            (Token::Identifier, true) => Color32::from_rgb(224, 192, 130),
            (Token::Identifier, false) => Color32::from_rgb(130, 85, 20),
            (Token::Number, true) => Color32::from_rgb(203, 159, 238),
            (Token::Number, false) => Color32::from_rgb(125, 50, 153),
        };
        job.append(
            &sql[offset..offset + length],
            0.0,
            TextFormat {
                font_id: FontId::monospace(14.0),
                color,
                italics: matches!(token, Token::Comment),
                ..Default::default()
            },
        );
        offset += length;
    }
    job
}

pub(super) fn next_token(text: &str) -> (usize, Token) {
    let bytes = text.as_bytes();
    if text.starts_with("--") || text.starts_with('#') {
        return (text.find('\n').unwrap_or(text.len()), Token::Comment);
    }
    if text.starts_with("/*") {
        let mut depth = 1;
        let mut i = 2;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"/*") {
                depth += 1;
                i += 2;
            } else if bytes[i..].starts_with(b"*/") {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    break;
                }
            } else {
                i += 1;
            }
        }
        return (i, Token::Comment);
    }
    if bytes[0] == b'$' {
        // PostgreSQL dollar strings: $$...$$ and $tag$...$tag$ (not $1).
        let mut i = 1;
        if bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            i += 1;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                i += 1;
            }
        }
        if bytes.get(i) == Some(&b'$') {
            let delimiter = &text[..i + 1];
            let start = i + 1;
            let end = text[start..]
                .find(delimiter)
                .map_or(text.len(), |end| start + end + delimiter.len());
            return (end, Token::String);
        }
    }
    if matches!(bytes[0], b'\'' | b'"' | b'`' | b'[') {
        let close = if bytes[0] == b'[' { b']' } else { bytes[0] };
        let mut i = 1;
        while i < bytes.len() {
            if bytes[i] == close {
                i += 1;
                if bytes.get(i) == Some(&close) {
                    i += 1;
                } else {
                    break;
                }
            } else if bytes[i] == b'\\' && close != b']' {
                i += 1;
                // Escape an entire Unicode scalar value, not just its first byte.
                if i < bytes.len() {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
            } else {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
        }
        return (
            i,
            if bytes[0] == b'\'' {
                Token::String
            } else {
                Token::Identifier
            },
        );
    }
    if bytes[0].is_ascii_digit()
        || (bytes[0] == b'.' && bytes.get(1).is_some_and(u8::is_ascii_digit))
    {
        let mut i = 1;
        if text.starts_with("0x") || text.starts_with("0X") {
            i = 2;
            while bytes.get(i).is_some_and(u8::is_ascii_hexdigit) {
                i += 1;
            }
        } else {
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
            {
                i += 1;
            }
            if bytes.get(i).is_some_and(|b| matches!(b, b'e' | b'E')) {
                let exponent = i;
                i += 1;
                if bytes.get(i).is_some_and(|b| matches!(b, b'+' | b'-')) {
                    i += 1;
                }
                let digits = i;
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == digits {
                    i = exponent;
                }
            }
        }
        return (i, Token::Number);
    }
    let first = text.chars().next().unwrap();
    if first.is_alphabetic() || first == '_' || first == '@' {
        let end = text
            .char_indices()
            .skip(1)
            .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '@')))
            .map_or(text.len(), |(i, _)| i);
        let token = if is_keyword(&text[..end]) {
            Token::Keyword
        } else {
            Token::Plain
        };
        return (end, token);
    }
    (first.len_utf8(), Token::Plain)
}

pub(super) fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word.to_ascii_uppercase().as_str())
}

pub(super) fn word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '$')
}
