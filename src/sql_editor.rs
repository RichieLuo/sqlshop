//! Stateless SQL editor helpers. Byte ranges are always UTF-8 boundaries; egui
//! cursor positions, in contrast, count Unicode scalar values (not bytes).
use crate::model::Engine;
use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};
use std::ops::{Range, RangeInclusive};

/// Select a semicolon-delimited statement at a Unicode scalar cursor.
/// Returned byte ranges include the terminator and retain comments. A cursor
/// on a terminator selects the preceding statement; trailing whitespace at EOF
/// also selects it. Comment-only segments have no executable statement.
/// This deliberately refuses procedural bodies and client batch directives.
/// It is a lexer, not a SQL parser, and validates the whole document first.
pub fn current_statement_range(
    sql: &str,
    cursor: usize,
    engine: Engine,
) -> Result<Option<Range<usize>>, String> {
    if cursor > sql.chars().count() {
        return Err("SQL 光标超出文本范围。".into());
    }
    let cursor = char_to_byte_index(sql, cursor);
    let bytes = sql.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut has_code = false;
    let mut line_has_code = false;
    let mut parentheses = 0usize;
    let mut words = Vec::new();
    while offset < sql.len() {
        let ch = sql[offset..].chars().next().unwrap();
        if ch.is_whitespace() {
            if matches!(ch, '\n' | '\r') {
                line_has_code = false;
            }
            offset += ch.len_utf8();
            continue;
        }
        let mysql_dash_comment = engine != Engine::MySql
            || bytes
                .get(offset + 2)
                .is_none_or(|b| b.is_ascii_whitespace() || *b < 32);
        if (bytes[offset..].starts_with(b"--") && mysql_dash_comment)
            || (engine == Engine::MySql && ch == '#')
        {
            offset = sql[offset..]
                .find(['\n', '\r'])
                .map_or(sql.len(), |n| offset + n);
            continue;
        }
        if bytes[offset..].starts_with(b"/*") {
            if engine == Engine::MySql
                && (bytes[offset..].starts_with(b"/*!") || bytes[offset..].starts_with(b"/*M!"))
            {
                return Err("可执行 MySQL 注释不能安全拆分，请使用执行选区或全文。".into());
            }
            let comment_start = offset;
            offset = statement_comment_end(sql, offset, engine)?;
            if sql[comment_start..offset].contains(['\n', '\r']) {
                line_has_code = false;
            }
            continue;
        }
        if ch == ';' {
            if parentheses > 0 {
                return Err("括号内的分号不能安全拆分，请显式选择完整语句或执行全文。".into());
            }
            reject_statement_body(&words, engine)?;
            segments.push((start..offset + 1, has_code));
            start = offset + 1;
            has_code = false;
            words.clear();
            line_has_code = true;
            offset += 1;
            continue;
        }
        if engine == Engine::Postgres
            && ch == '$'
            && (offset == 0 || !sql[..offset].chars().next_back().is_some_and(word_char))
            && let Some(delimiter) = statement_dollar_delimiter(&sql[offset..])
        {
            let body_start = offset + delimiter.len();
            offset = sql[body_start..]
                .find(delimiter)
                .map(|n| body_start + n + delimiter.len())
                .ok_or_else(|| "PostgreSQL dollar quote 尚未闭合。".to_owned())?;
            has_code = true;
            line_has_code = true;
            continue;
        }
        let quoted = matches!(ch, '\'' | '"')
            || (engine == Engine::MySql && ch == '`')
            || (engine == Engine::SqlServer && ch == '[');
        if quoted {
            let postgres_escape = engine == Engine::Postgres
                && ch == '\''
                && offset > 0
                && matches!(bytes[offset - 1], b'e' | b'E')
                && (offset == 1 || !sql[..offset - 1].chars().next_back().is_some_and(word_char));
            let ambiguous_backslash = engine == Engine::MySql
                || (engine == Engine::Postgres && ch == '\'' && !postgres_escape);
            offset = statement_quote_end(sql, offset, postgres_escape, ambiguous_backslash)?;
            has_code = true;
            line_has_code = true;
            continue;
        }
        if ch.is_alphabetic() || matches!(ch, '_' | '@') {
            let length = sql[offset..]
                .char_indices()
                .skip(1)
                .find(|(_, c)| !(word_char(*c) || *c == '@'))
                .map_or(sql.len() - offset, |(n, _)| n);
            let word = sql[offset..offset + length].to_ascii_uppercase();
            if !line_has_code
                && ((engine == Engine::SqlServer && word == "GO")
                    || (engine == Engine::MySql && word == "DELIMITER"))
            {
                return Err(format!(
                    "{word} 批处理指令不能安全拆分，请使用执行选区或全文。"
                ));
            }
            words.push(word);
            offset += length;
        } else {
            if ch == '(' {
                parentheses += 1;
            } else if ch == ')' {
                parentheses = parentheses.saturating_sub(1);
            }
            offset += ch.len_utf8();
        }
        has_code = true;
        line_has_code = true;
    }
    reject_statement_body(&words, engine)?;
    segments.push((start..sql.len(), has_code));
    let index = segments
        .iter()
        .position(|(range, _)| cursor < range.end)
        .unwrap_or(segments.len() - 1);
    let (mut range, mut has_code) = segments[index].clone();
    if !has_code && sql[range.clone()].trim().is_empty() && index > 0 {
        (range, has_code) = segments[index - 1].clone();
    }
    if !has_code {
        return Ok(None);
    }
    let text = &sql[range.clone()];
    range.start += text.len() - text.trim_start().len();
    range.end -= text.len() - text.trim_end().len();
    Ok(Some(range))
}

fn statement_comment_end(sql: &str, mut offset: usize, engine: Engine) -> Result<usize, String> {
    let mut depth = 1;
    offset += 2;
    while offset < sql.len() {
        if sql.as_bytes()[offset..].starts_with(b"/*") {
            if engine == Engine::MySql {
                return Err("MySQL 嵌套块注释不能安全拆分，请使用执行选区或全文。".into());
            }
            depth += 1;
            offset += 2;
        } else if sql.as_bytes()[offset..].starts_with(b"*/") {
            depth -= 1;
            offset += 2;
            if depth == 0 {
                return Ok(offset);
            }
        } else {
            offset += sql[offset..].chars().next().unwrap().len_utf8();
        }
    }
    Err("SQL 块注释尚未闭合。".into())
}

fn statement_dollar_delimiter(text: &str) -> Option<&str> {
    let mut chars = text.char_indices().skip(1);
    let (_, first) = chars.next()?;
    if first == '$' {
        return Some(&text[..2]);
    }
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    for (offset, ch) in chars {
        if ch == '$' {
            return Some(&text[..offset + 1]);
        }
        if !(ch.is_alphanumeric() || ch == '_') {
            return None;
        }
    }
    None
}

fn statement_quote_end(
    sql: &str,
    start: usize,
    backslash_escape: bool,
    ambiguous_backslash: bool,
) -> Result<usize, String> {
    let bytes = sql.as_bytes();
    let close = if bytes[start] == b'[' {
        b']'
    } else {
        bytes[start]
    };
    let mut offset = start + 1;
    while offset < sql.len() {
        if ambiguous_backslash && bytes[offset] == b'\\' {
            return Err(
                "引用中的反斜杠受会话 SQL 模式影响，无法确认语句边界，请使用执行选区或全文。"
                    .into(),
            );
        }
        if bytes[offset] == close {
            offset += 1;
            if bytes.get(offset) == Some(&close) {
                offset += 1;
            } else {
                return Ok(offset);
            }
        } else if backslash_escape && bytes[offset] == b'\\' {
            offset += 1;
            if offset < sql.len() {
                offset += sql[offset..].chars().next().unwrap().len_utf8();
            }
        } else {
            offset += sql[offset..].chars().next().unwrap().len_utf8();
        }
    }
    Err("SQL 字符串或引用标识符尚未闭合。".into())
}

fn reject_statement_body(words: &[String], engine: Engine) -> Result<(), String> {
    let first = words.first().map(String::as_str).unwrap_or("");
    let definition = words
        .iter()
        .any(|word| matches!(word.as_str(), "CREATE" | "ALTER"))
        && words.iter().any(|word| {
            matches!(
                word.as_str(),
                "PROCEDURE" | "PROC" | "FUNCTION" | "TRIGGER" | "EVENT" | "RULE"
            )
        });
    let control_flow = matches!(
        first,
        "DO" | "DECLARE"
            | "IF"
            | "WHILE"
            | "LOOP"
            | "REPEAT"
            | "CASE"
            | "END"
            | "ELSE"
            | "ELSIF"
            | "GOTO"
            | "RETURN"
            | "LEAVE"
            | "ITERATE"
    ) || words
        .iter()
        .any(|word| matches!(word.as_str(), "DECLARE" | "WHILE" | "LOOP" | "REPEAT"))
        || (engine == Engine::SqlServer
            && words
                .iter()
                .any(|word| matches!(word.as_str(), "IF" | "GOTO" | "RETURN")))
        || words.iter().enumerate().any(|(index, word)| {
            word == "BEGIN"
                && !words
                    .get(index + 1)
                    .is_some_and(|word| matches!(word.as_str(), "TRANSACTION" | "TRAN" | "WORK"))
                && !(words.len() == 1 && engine != Engine::SqlServer)
        });
    if definition || control_flow {
        return Err("过程定义或控制流不能安全拆分，请显式选择完整语句或执行全文。".into());
    }
    Ok(())
}

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

pub const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "TABLE",
    "VIEW",
    "INDEX",
    "DATABASE",
    "SCHEMA",
    "USE",
    "SHOW",
    "DESCRIBE",
    "EXPLAIN",
    "ANALYZE",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "TRUE",
    "FALSE",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ON",
    "USING",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "FETCH",
    "NEXT",
    "ROWS",
    "ONLY",
    "TOP",
    "DISTINCT",
    "ALL",
    "ASC",
    "DESC",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "RECURSIVE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "EXISTS",
    "IN",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "ESCAPE",
    "OVER",
    "PARTITION",
    "RETURNING",
    "OUTPUT",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "SAVEPOINT",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "CONSTRAINT",
    "DEFAULT",
    "UNIQUE",
    "CHECK",
    "IF",
    "REPLACE",
    "CONFLICT",
    "DO",
    "NOTHING",
    "DECLARE",
    "EXEC",
    "EXECUTE",
    "PROCEDURE",
    "FUNCTION",
    "GO",
    "CAST",
    "CONVERT",
];

fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word.to_ascii_uppercase().as_str())
}

fn word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '$')
}

/// The editable word under a character cursor; ignore comments, quoted text and selections.
pub fn completion_word(sql: &str, cursor: usize) -> Option<Range<usize>> {
    if cursor > sql.chars().count() {
        return None;
    }
    let end = char_to_byte_index(sql, cursor);
    if end == 0 || !sql[..end].chars().next_back().is_some_and(word_char) {
        return None;
    }
    let start = sql[..end]
        .char_indices()
        .rev()
        .find(|(_, ch)| !word_char(*ch))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    if sql[start..end]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_numeric() || ch == '$')
    {
        return None;
    }
    let mut finish = end;
    while let Some(ch) = sql[finish..].chars().next() {
        if !word_char(ch) {
            break;
        }
        finish += ch.len_utf8();
    }
    let mut offset = 0;
    while offset < end {
        let (length, token) = next_token(&sql[offset..]);
        if offset + length > start {
            if offset != start || !matches!(token, Token::Plain | Token::Keyword) {
                return None;
            }
            return Some(start..finish);
        }
        offset += length;
    }
    None
}

pub fn completions(prefix: &str, columns: &[String]) -> Vec<(String, bool)> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for keyword in KEYWORDS {
        if keyword.len() > prefix.len()
            && keyword
                .to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
        {
            candidates.push(((*keyword).to_owned(), false));
        }
    }
    for column in columns {
        if column.len() > prefix.len()
            && column.to_lowercase().starts_with(&prefix.to_lowercase())
            && !candidates
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(column))
        {
            candidates.push((column.clone(), true));
        }
    }
    candidates.sort_by_key(|a| a.0.to_lowercase());
    candidates.truncate(12);
    candidates
}

pub fn insert_completion(sql: &mut String, range: Range<usize>, value: &str) -> Option<usize> {
    sql.get(range.clone())?;
    let cursor = byte_to_char_index(sql, range.start) + value.chars().count();
    sql.replace_range(range, value);
    Some(cursor)
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

    fn statement_at(sql: &str, byte_cursor: usize, engine: Engine) -> Option<&str> {
        let range =
            current_statement_range(sql, byte_to_char_index(sql, byte_cursor), engine).unwrap();
        range.map(|range| &sql[range])
    }

    #[test]
    fn statement_ranges_respect_unicode_cursor_and_semicolon_boundaries() {
        let sql = "  SELECT '中文🦀';\n SELECT 2;  ";
        let delimiter = sql.find(';').unwrap();
        for engine in Engine::ALL {
            assert_eq!(statement_at(sql, 0, engine), Some("SELECT '中文🦀';"));
            assert_eq!(
                statement_at(sql, delimiter, engine),
                Some("SELECT '中文🦀';")
            );
            assert_eq!(statement_at(sql, delimiter + 1, engine), Some("SELECT 2;"));
            assert_eq!(statement_at(sql, sql.len(), engine), Some("SELECT 2;"));
            assert_eq!(statement_at("SELECT 1", 8, engine), Some("SELECT 1"));
            assert!(current_statement_range(sql, usize::MAX, engine).is_err());
        }
    }

    #[test]
    fn statement_ranges_ignore_quoted_semicolons_and_dialect_comments() {
        let cases = [
            (
                Engine::MySql,
                "SELECT `中;``文`, 'a;''b', \"c;\"; #尾;\nSELECT 2;",
            ),
            (
                Engine::Postgres,
                "SELECT \"中;\"\"文\", 'a;''b', E'\\\'🦀;'; --尾;\nSELECT 2;",
            ),
            (
                Engine::SqlServer,
                "SELECT [中;]]文], N'a;''b' FROM #temp; --尾;\nSELECT 2;",
            ),
        ];
        for (engine, sql) in cases {
            let end = sql.find("; ").unwrap() + 1;
            assert_eq!(
                statement_at(sql, sql.find('中').unwrap(), engine),
                Some(&sql[..end])
            );
            assert!(
                statement_at(sql, sql.len(), engine)
                    .unwrap()
                    .ends_with("SELECT 2;")
            );
        }
        let sql = "SELECT 1--2;SELECT 3;";
        assert_eq!(statement_at(sql, 0, Engine::MySql), Some("SELECT 1--2;"));
        assert_eq!(statement_at(sql, 0, Engine::Postgres), Some(sql));
        let sql = "SELECT 1;--comment\rSELECT 2;";
        assert_eq!(
            statement_at(sql, sql.len(), Engine::SqlServer),
            Some("--comment\rSELECT 2;")
        );
    }

    #[test]
    fn statement_ranges_support_nested_comments_and_postgres_dollar_strings() {
        let sql = "SELECT /* 外; /* 内; */ 尾 */ $标签$中;🦀$标签$, $$a;b$$, $1; SELECT 2;";
        let end = sql.find("; SELECT").unwrap() + 1;
        assert_eq!(
            statement_at(sql, sql.find('🦀').unwrap(), Engine::Postgres),
            Some(&sql[..end])
        );
        let sql = "SELECT /* 外; /* 内; */ 尾 */ 1; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, Engine::SqlServer),
            Some("SELECT /* 外; /* 内; */ 尾 */ 1;")
        );
        assert!(current_statement_range(sql, 0, Engine::MySql).is_err());
        let sql = "SELECT a$tag$; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, Engine::Postgres),
            Some("SELECT a$tag$;")
        );
    }

    #[test]
    fn statement_ranges_do_not_execute_empty_or_comment_only_segments() {
        for engine in Engine::ALL {
            for sql in ["", " \n ", "; ;", "-- 无 SQL;", "/* 只有注释; */"] {
                assert_eq!(statement_at(sql, sql.len(), engine), None);
            }
            let sql = "SELECT 1; -- 只有注释;";
            assert_eq!(statement_at(sql, sql.len(), engine), None);
        }
    }

    #[test]
    fn statement_ranges_refuse_unclosed_quotes_and_ambiguous_session_modes() {
        for engine in Engine::ALL {
            for sql in [
                "SELECT 1; SELECT '未结束;",
                "SELECT \"未结束;",
                "SELECT /* 未结束;",
            ] {
                assert!(
                    current_statement_range(sql, 0, engine).is_err(),
                    "{engine:?}: {sql}"
                );
            }
        }
        for sql in ["SELECT `未结束;", "SELECT 'a\\b';", "SELECT \"a\\b\";"] {
            assert!(current_statement_range(sql, 0, Engine::MySql).is_err());
        }
        assert!(current_statement_range("SELECT [未结束;", 0, Engine::SqlServer).is_err());
        assert!(current_statement_range("SELECT $tag$未结束;", 0, Engine::Postgres).is_err());
        assert!(current_statement_range("SELECT 'a\\b';", 0, Engine::Postgres).is_err());
        let sql = "SELECT E'a\\b'; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, Engine::Postgres),
            Some("SELECT E'a\\b';")
        );
        let sql = "SELECT 'a\\b'; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, Engine::SqlServer),
            Some("SELECT 'a\\b';")
        );
    }

    #[test]
    fn statement_ranges_refuse_procedural_batches_but_preserve_case_expressions() {
        for (engine, sql) in [
            (
                Engine::MySql,
                "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$",
            ),
            (
                Engine::MySql,
                "CREATE DEFINER='u'@'h' PROCEDURE p() BEGIN SELECT 1; END;",
            ),
            (Engine::MySql, "SELECT 1; /*! SELECT 2; */ SELECT 3;"),
            (
                Engine::Postgres,
                "CREATE OR REPLACE FUNCTION f() RETURNS int AS $$SELECT 1;$$ LANGUAGE SQL;",
            ),
            (Engine::Postgres, "DO $$BEGIN SELECT 1; END;$$;"),
            (
                Engine::SqlServer,
                "SELECT 1;\n/* comment */ GO 2 -- batch\nSELECT 2;",
            ),
            (
                Engine::SqlServer,
                "CREATE OR ALTER PROC p AS SELECT 1; SELECT 2;",
            ),
            (Engine::SqlServer, "BEGIN TRY SELECT 1; END TRY;"),
            (Engine::SqlServer, "DECLARE @x int = 1; SELECT @x;"),
            (Engine::SqlServer, "IF 1=1 SELECT 1; ELSE SELECT 2;"),
            (Engine::SqlServer, "SELECT 1\nDECLARE @x int=1; SELECT @x;"),
            (Engine::SqlServer, "SELECT 1\nBEGIN TRY SELECT 2; END TRY;"),
            (Engine::SqlServer, "SELECT 1\nIF 1=1 SELECT 2; SELECT 3;"),
            (
                Engine::SqlServer,
                "SELECT 1\nGOTO finished; SELECT 2; finished: SELECT 3;",
            ),
            (Engine::SqlServer, "SELECT 1; finished: RETURN; SELECT 2;"),
            (
                Engine::Postgres,
                "CREATE RULE r AS ON UPDATE TO t DO ALSO (UPDATE a SET x=1; UPDATE b SET x=2;);",
            ),
            (Engine::Postgres, "SELECT (1; DELETE FROM t;);"),
        ] {
            assert!(
                current_statement_range(sql, 0, engine).is_err(),
                "{engine:?}: {sql}"
            );
        }
        for engine in Engine::ALL {
            let sql = "SELECT CASE WHEN x=1 THEN 'a;b' ELSE 'c' END FROM t; SELECT 2;";
            assert_eq!(
                statement_at(sql, 0, engine),
                Some("SELECT CASE WHEN x=1 THEN 'a;b' ELSE 'c' END FROM t;")
            );
            assert_eq!(
                statement_at("BEGIN TRANSACTION; SELECT 1;", 0, engine),
                Some("BEGIN TRANSACTION;")
            );
            let sql = "SELECT 'GO; DELIMITER; CREATE PROCEDURE';";
            assert_eq!(statement_at(sql, 0, engine), Some(sql));
        }
        let sql = "SELECT IF(x=1, 'a;b', 'c') FROM t; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, Engine::MySql),
            Some("SELECT IF(x=1, 'a;b', 'c') FROM t;")
        );
    }

    #[test]
    fn completion_matches_words_and_preserves_unicode_cursor() {
        let sql = "🦀 SELECT 中名尾 FROM t";
        let cursor = "🦀 SELECT 中".chars().count();
        let range = completion_word(sql, cursor).unwrap();
        assert_eq!(&sql[range.clone()], "中名尾");
        let choices = completions("中", &["中文列".into(), "中文列".into(), "其他".into()]);
        assert_eq!(choices, [("中文列".into(), true)]);
        let mut text = sql.to_owned();
        let cursor = insert_completion(&mut text, range, &choices[0].0).unwrap();
        assert_eq!(text, "🦀 SELECT 中文列 FROM t");
        assert_eq!(cursor, "🦀 SELECT 中文列".chars().count());
        assert_eq!(completions("sel", &[]), [("SELECT".into(), false)]);
        assert!(completion_word("-- SELECT", 9).is_none());
        assert!(completion_word("SELECT 'abc'", 11).is_none());
        assert!(completion_word("SELECT `abc`", 11).is_none());
        assert!(completion_word("/* abc */", 6).is_none());
        assert!(completion_word("SELECT 123", 10).is_none());
        assert!(completion_word("abc$def", 7).is_some());
        assert!(completion_word("SELECT abc", usize::MAX).is_none());
        assert!(insert_completion(&mut text, 1..3, "x").is_none());
    }

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
