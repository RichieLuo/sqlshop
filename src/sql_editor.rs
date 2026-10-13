//! Stateless SQL editor helpers. Byte ranges are always UTF-8 boundaries; egui
//! cursor positions, in contrast, count Unicode scalar values (not bytes).
use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};
use std::ops::{Range, RangeInclusive};

#[derive(Clone, Copy)]
enum Token {
    Plain,
    Keyword,
    String,
    Comment,
    Identifier,
    Number,
}

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

fn next_token(text: &str) -> (usize, Token) {
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

fn is_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "SELECT"
            | "FROM"
            | "WHERE"
            | "INSERT"
            | "INTO"
            | "VALUES"
            | "UPDATE"
            | "SET"
            | "DELETE"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TRUNCATE"
            | "TABLE"
            | "VIEW"
            | "INDEX"
            | "DATABASE"
            | "SCHEMA"
            | "USE"
            | "SHOW"
            | "DESCRIBE"
            | "EXPLAIN"
            | "ANALYZE"
            | "AS"
            | "AND"
            | "OR"
            | "NOT"
            | "NULL"
            | "IS"
            | "TRUE"
            | "FALSE"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "OUTER"
            | "CROSS"
            | "ON"
            | "USING"
            | "GROUP"
            | "BY"
            | "ORDER"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "FETCH"
            | "NEXT"
            | "ROWS"
            | "ONLY"
            | "TOP"
            | "DISTINCT"
            | "ALL"
            | "ASC"
            | "DESC"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "WITH"
            | "RECURSIVE"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "EXISTS"
            | "IN"
            | "BETWEEN"
            | "LIKE"
            | "ILIKE"
            | "ESCAPE"
            | "OVER"
            | "PARTITION"
            | "RETURNING"
            | "OUTPUT"
            | "BEGIN"
            | "COMMIT"
            | "ROLLBACK"
            | "TRANSACTION"
            | "SAVEPOINT"
            | "PRIMARY"
            | "KEY"
            | "FOREIGN"
            | "REFERENCES"
            | "CONSTRAINT"
            | "DEFAULT"
            | "UNIQUE"
            | "CHECK"
            | "IF"
            | "REPLACE"
            | "CONFLICT"
            | "DO"
            | "NOTHING"
            | "DECLARE"
            | "EXEC"
            | "EXECUTE"
            | "PROCEDURE"
            | "FUNCTION"
            | "GO"
            | "CAST"
            | "CONVERT"
    )
}

/// Non-overlapping matches. Empty needles have no matches. Case-insensitive
/// matching uses Unicode lowercase, not locale-specific or full case folding.
pub fn find_matches(text: &str, needle: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        return text
            .match_indices(needle)
            .map(|(start, s)| start..start + s.len())
            .collect();
    }
    let mut folded = String::new();
    let mut boundaries = vec![(0, 0)];
    for (index, ch) in text.char_indices() {
        folded.extend(ch.to_lowercase());
        boundaries.push((folded.len(), index + ch.len_utf8()));
    }
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    let mut matches = Vec::new();
    let mut previous_end = 0;
    // Iterate original scalar boundaries so a lowercase expansion cannot create
    // a match for half a source character or hide a later valid match.
    for &(folded_start, original_start) in &boundaries {
        if original_start < previous_end || !folded[folded_start..].starts_with(&needle) {
            continue;
        }
        let folded_end = folded_start + needle.len();
        if let Ok(index) = boundaries.binary_search_by_key(&folded_end, |&(folded, _)| folded) {
            let original_end = boundaries[index].1;
            matches.push(original_start..original_end);
            previous_end = original_end;
        }
    }
    matches
}

pub fn replace_match(text: &mut String, range: Range<usize>, replacement: &str) -> bool {
    if range.start > range.end || text.get(range.clone()).is_none() {
        return false;
    }
    text.replace_range(range, replacement);
    true
}

pub fn replace_all(
    text: &str,
    needle: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    let matches = find_matches(text, needle, case_sensitive);
    let mut result = String::with_capacity(text.len());
    let mut previous = 0;
    for range in &matches {
        result.push_str(&text[previous..range.start]);
        result.push_str(replacement);
        previous = range.end;
    }
    result.push_str(&text[previous..]);
    (result, matches.len())
}

/// Clamp out-of-range byte indices; a byte inside a character rounds down.
pub fn byte_to_char_index(text: &str, byte_index: usize) -> usize {
    let mut index = byte_index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    text[..index].chars().count()
}

pub fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(index, _)| index)
}

/// Copy a rectangular cell selection with spreadsheet-compatible TSV quoting.
/// Missing cells in ragged rows become empty fields; out-of-bounds ranges clip.
pub fn copy_tsv(
    rows: &[Vec<String>],
    row_range: RangeInclusive<usize>,
    column_range: RangeInclusive<usize>,
) -> String {
    if row_range.is_empty() || column_range.is_empty() {
        return String::new();
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let start_col = *column_range.start();
    if start_col >= width {
        return String::new();
    }
    let end_col = (*column_range.end()).min(width - 1);
    let mut output = String::new();
    for (index, row) in rows.iter().enumerate().skip(*row_range.start()).take(
        row_range
            .end()
            .saturating_sub(*row_range.start())
            .saturating_add(1),
    ) {
        if index > *row_range.start() {
            output.push('\n');
        }
        for column in start_col..=end_col {
            if column > start_col {
                output.push('\t');
            }
            let value = row.get(column).map(String::as_str).unwrap_or("");
            if value.contains(['\t', '\n', '\r', '"']) {
                output.push('"');
                output.push_str(&value.replace('"', "\"\""));
                output.push('"');
            } else {
                output.push_str(value);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_find_replace_and_cursor_indices() {
        let text = "中文 🦀 SELECT 名, select 'Ééİ';";
        let found = find_matches(text, "select", false);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|range| text.get(range.clone()).is_some()));
        assert_eq!(
            replace_all(text, "select", "查找", false).0,
            "中文 🦀 查找 名, 查找 'Ééİ';"
        );
        assert_eq!(find_matches(text, "é", false).len(), 2);
        assert!(find_matches("İ", "i", false).is_empty());
        assert_eq!(find_matches("İ", "i\u{307}", false), vec![0..2]);
        assert!(find_matches(text, "", true).is_empty());
        let mut value = "中🦀文".to_owned();
        assert!(!replace_match(&mut value, 1..3, "bad"));
        assert!(replace_match(&mut value, 3..7, "好"));
        assert_eq!(value, "中好文");
        assert_eq!(byte_to_char_index("中🦀文", 5), 1);
        assert_eq!(char_to_byte_index("中🦀文", 2), 7);
        assert_eq!(char_to_byte_index("中🦀文", usize::MAX), 10);
    }

    #[test]
    fn replacements_do_not_reprocess_inserted_text() {
        assert_eq!(replace_all("aaa", "aa", "aaaa", true), ("aaaaa".into(), 1));
        assert_eq!(replace_all("中文", "", "x", false), ("中文".into(), 0));
    }

    #[test]
    fn highlights_dialects_without_changing_text_or_splitting_unicode() {
        let inputs = [
            "SELECT `名``称`, [a]]b], \"字段\" FROM t WHERE x = 'it''s 🦀'; --注释\n",
            "SELECT $tag$你好 'x'$tag$, $$🦀$$, 1.2e-3, 0xAB; /* outer /* 中 */ end */",
            "# 中文\nSELECT E'\\中', 'unfinished 🦀",
            "/* 未结束 🦀",
            "[未结束",
            "SELECT 🦀",
            "",
            "'\\🦀'",
        ];
        for input in inputs {
            for dark in [false, true] {
                let job = highlight_sql(input, dark);
                assert_eq!(job.text, input);
                let mut end = 0;
                for section in &job.sections {
                    assert_eq!(section.byte_range.start, end);
                    assert!(input.get(section.byte_range.clone()).is_some());
                    end = section.byte_range.end;
                }
                assert_eq!(end, input.len());
            }
        }
        let job = highlight_sql("SELECT 'str' -- comment", true);
        assert_ne!(job.sections[0].format.color, job.sections[2].format.color);
        assert!(job.sections.last().unwrap().format.italics);
    }

    #[test]
    fn tsv_handles_rectangle_quoting_and_ragged_rows() {
        let rows = vec![
            vec!["id".into(), "名".into(), "tail".into()],
            vec!["1".into(), "a\tb\n\"c\"".into()],
            vec!["2".into()],
        ];
        assert_eq!(
            copy_tsv(&rows, 0..=2, 1..=2),
            "名\ttail\n\"a\tb\n\"\"c\"\"\"\t\n\t"
        );
        assert_eq!(copy_tsv(&rows, 8..=usize::MAX, 0..=2), "");
        assert_eq!(copy_tsv(&rows, 0..=2, 9..=usize::MAX), "");
        assert_eq!(copy_tsv(&[], 0..=0, 0..=0), "");
    }
}
