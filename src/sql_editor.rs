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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletionKind {
    Keyword,
    Field,
    Table,
}

impl CompletionKind {
    pub fn label(self) -> &'static str {
        match self {
            CompletionKind::Keyword => "关键字",
            CompletionKind::Field => "字段",
            CompletionKind::Table => "表",
        }
    }
}

/// Completion candidates for the word under the cursor: SQL keywords, then
/// column names, then table names. Keywords win name collisions, then
/// columns, so a column named like a keyword is not offered twice.
pub fn completions(
    prefix: &str,
    columns: &[String],
    tables: &[String],
) -> Vec<(String, CompletionKind)> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let folded_prefix = prefix.to_lowercase();
    let mut candidates: Vec<(String, CompletionKind)> = Vec::new();
    for keyword in KEYWORDS {
        if keyword.len() > prefix.len() && keyword.to_ascii_lowercase().starts_with(&folded_prefix)
        {
            candidates.push(((*keyword).to_owned(), CompletionKind::Keyword));
        }
    }
    for column in columns {
        if column.len() > prefix.len()
            && column.to_lowercase().starts_with(&folded_prefix)
            && !candidates
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(column))
        {
            candidates.push((column.clone(), CompletionKind::Field));
        }
    }
    for table in tables {
        if table.len() > prefix.len()
            && table.to_lowercase().starts_with(&folded_prefix)
            && !candidates
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(table))
        {
            candidates.push((table.clone(), CompletionKind::Table));
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

#[derive(Clone, Debug)]
struct SqlLexeme<'a> {
    text: &'a str,
    range: Range<usize>,
    opaque: bool,
    comment: bool,
}

// The dialect lexer deliberately refuses ambiguous session-dependent quoting.
// Formatting edits whitespace only, retaining literal prefixes and operators.
fn editing_tokens(sql: &str, engine: Engine) -> Result<Vec<SqlLexeme<'_>>, String> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < sql.len() {
        let start = offset;
        let ch = sql[offset..].chars().next().unwrap();
        if ch.is_whitespace() {
            offset += ch.len_utf8();
            continue;
        }
        let line_comment = (sql[offset..].starts_with("--")
            && (engine != Engine::MySql
                || sql
                    .as_bytes()
                    .get(offset + 2)
                    .is_none_or(|b| b.is_ascii_whitespace() || *b < 32)))
            || (engine == Engine::MySql && ch == '#');
        let block_comment = sql[offset..].starts_with("/*");
        let quoted = matches!(ch, '\'' | '"')
            || (engine == Engine::MySql && ch == '`')
            || (engine == Engine::SqlServer && ch == '[');
        let dollar = if engine == Engine::Postgres && ch == '$' {
            statement_dollar_delimiter(&sql[offset..])
        } else {
            None
        };
        if line_comment {
            offset = sql[offset..]
                .find(['\n', '\r'])
                .map_or(sql.len(), |n| offset + n);
        } else if block_comment {
            offset = statement_comment_end(sql, offset, engine)?;
        } else if let Some(delimiter) = dollar {
            let body = offset + delimiter.len();
            offset = sql[body..]
                .find(delimiter)
                .map(|n| body + n + delimiter.len())
                .ok_or("PostgreSQL dollar quote 尚未闭合。")?;
        } else if quoted {
            let escape = engine == Engine::Postgres
                && ch == '\''
                && offset > 0
                && matches!(sql.as_bytes()[offset - 1], b'E' | b'e')
                && (offset == 1 || !sql[..offset - 1].chars().next_back().is_some_and(word_char));
            offset = statement_quote_end(
                sql,
                offset,
                escape,
                engine == Engine::MySql || (engine == Engine::Postgres && ch == '\'' && !escape),
            )?;
        } else if word_char(ch) || ch == '@' {
            offset += ch.len_utf8();
            while let Some(ch) = sql[offset..].chars().next() {
                if !word_char(ch) && ch != '@' {
                    break;
                }
                offset += ch.len_utf8();
            }
        } else {
            offset += ch.len_utf8();
        }
        tokens.push(SqlLexeme {
            text: &sql[start..offset],
            range: start..offset,
            opaque: quoted || dollar.is_some(),
            comment: line_comment || block_comment,
        });
    }
    Ok(tokens)
}

/// A conservative whitespace formatter. Quoted values, comments, identifier
/// spelling, and token adjacency are preserved exactly. This never parses or
/// rewrites SQL expressions. Session-dependent ambiguous quotes are refused.
pub fn format_sql(sql: &str, engine: Engine, compact: bool) -> Result<String, String> {
    let tokens = editing_tokens(sql, engine)?;
    let line_start = |offset: usize| {
        sql[..offset]
            .rfind(['\n', '\r'])
            .map_or(0, |index| index + 1)
    };
    let batch_lines: std::collections::HashSet<_> = tokens
        .iter()
        .filter(|token| {
            !token.opaque
                && !token.comment
                && (token.text.eq_ignore_ascii_case("GO")
                    || token.text.eq_ignore_ascii_case("DELIMITER"))
        })
        .map(|token| line_start(token.range.start))
        .collect();
    let mut output = String::new();
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            let previous = &tokens[index - 1];
            let gap = &sql[previous.range.end..token.range.start];
            // Never split adjacent tokens: E'...', N'...', ::, ->>, numbers,
            // executable comments and dialect operators must retain spelling.
            if !gap.is_empty() {
                let line_comment = previous.comment && !previous.text.starts_with("/*");
                let batch_line = batch_lines.contains(&line_start(previous.range.end))
                    || batch_lines.contains(&line_start(token.range.start));
                // PostgreSQL concatenates adjacent string constants only when
                // their separating whitespace contains a newline. Preserve it
                // at literal boundaries even in compact mode (including when
                // a comment or literal prefix sits between the constants).
                let literal_newline = engine == Engine::Postgres
                    && gap.contains(['\n', '\r'])
                    && (previous.text.starts_with('\'') || token.text.starts_with('\''));
                let clause = !token.opaque
                    && !token.comment
                    && matches!(
                        token.text.to_ascii_uppercase().as_str(),
                        "SELECT"
                            | "FROM"
                            | "WHERE"
                            | "HAVING"
                            | "LIMIT"
                            | "OFFSET"
                            | "RETURNING"
                            | "UNION"
                            | "EXCEPT"
                            | "INTERSECT"
                            | "SET"
                            | "VALUES"
                            | "JOIN"
                            | "LEFT"
                            | "RIGHT"
                            | "INNER"
                            | "FULL"
                            | "GROUP"
                            | "ORDER"
                    );
                if line_comment
                    || literal_newline
                    || (batch_line && gap.contains(['\n', '\r']))
                    || (!compact && (clause || previous.text == ";" || previous.text == ","))
                {
                    output.push('\n');
                    if !compact {
                        output.push_str(&"    ".repeat(depth.min(16)));
                    }
                } else {
                    output.push(' ');
                }
            }
        }
        output.push_str(token.text);
        if !token.opaque && !token.comment {
            if token.text == "(" {
                depth += 1;
            }
            if token.text == ")" {
                depth = depth.saturating_sub(1);
            }
        }
    }
    Ok(output)
}

#[derive(Clone, Debug)]
pub struct CompletionTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<String>,
}

/// Context completion uses only metadata for this editor's bound connection.
/// Aliases are resolved inside the current statement and visible parent scopes;
/// sibling subqueries and previous statements cannot leak alias bindings.
pub type SemanticCompletions = (Range<usize>, Vec<(String, CompletionKind)>);

pub fn semantic_completions(
    sql: &str,
    cursor: usize,
    engine: Engine,
    tables: &[CompletionTable],
    fallback_columns: &[String],
) -> Option<SemanticCompletions> {
    if cursor > sql.chars().count() {
        return None;
    }
    let end = char_to_byte_index(sql, cursor);
    let tokens = editing_tokens(sql, engine).ok()?;
    let token = tokens.iter().find(|token| {
        token.range.start < end && token.range.end >= end && !token.opaque && !token.comment
    })?;
    let range = if token.text == "." && token.range.end == end {
        end..end
    } else if token.text.chars().all(word_char)
        && token
            .text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
    {
        token.range.clone()
    } else {
        return None;
    };
    let prefix = &sql[range.start..end];
    let significant: Vec<_> = tokens.iter().filter(|token| !token.comment).collect();
    // The nearest containing parenthesis scope at the cursor, plus ancestors.
    let mut scope = Vec::new();
    let mut scopes = Vec::new();
    let mut statement = 0usize;
    let mut cursor_scope = Vec::new();
    let mut cursor_statement = 0usize;
    for (index, token) in significant.iter().enumerate() {
        if token.range.start <= end {
            cursor_scope = scope.clone();
            cursor_statement = statement;
        }
        scopes.push((statement, scope.clone()));
        if !token.opaque {
            match token.text {
                "(" => scope.push(index),
                ")" => {
                    scope.pop();
                }
                ";" => {
                    statement += 1;
                    scope.clear();
                }
                _ => {}
            }
        }
    }
    let unquote = |text: &str| -> String {
        if text.starts_with('"') {
            text[1..text.len() - 1].replace("\"\"", "\"")
        } else if text.starts_with('`') {
            text[1..text.len() - 1].replace("``", "`")
        } else if text.starts_with('[') {
            text[1..text.len() - 1].replace("]]", "]")
        } else if engine == Engine::Postgres {
            text.to_lowercase()
        } else {
            text.to_owned()
        }
    };
    let identifier_matches = |left: &str, right: &str| {
        if engine == Engine::Postgres {
            left == right
        } else {
            left.eq_ignore_ascii_case(right)
        }
    };
    let qualifier = sql[..range.start].strip_suffix('.').and_then(|text| {
        let token = significant
            .iter()
            .rev()
            .find(|token| token.range.end <= text.len())?;
        (token.range.end == text.len()).then(|| unquote(token.text))
    });
    let mut aliases: Vec<(String, String, Option<String>, usize)> = Vec::new();
    for (index, token) in significant.iter().enumerate() {
        let (statement, scope) = &scopes[index];
        if *statement != cursor_statement
            || !cursor_scope.starts_with(scope)
            || token.opaque
            || !matches!(
                token.text.to_ascii_uppercase().as_str(),
                "FROM" | "JOIN" | "UPDATE" | "INTO"
            )
        {
            continue;
        }
        let mut next = index + 1;
        let Some(table) = significant.get(next) else {
            continue;
        };
        if table.text == "(" {
            continue;
        }
        let mut name = unquote(table.text);
        let mut schema = None;
        next += 1;
        if significant.get(next).is_some_and(|token| token.text == ".") {
            schema = Some(name);
            next += 1;
            let Some(table) = significant.get(next) else {
                continue;
            };
            name = unquote(table.text);
            next += 1;
        }
        aliases.push((name.clone(), name.clone(), schema.clone(), scope.len()));
        if significant
            .get(next)
            .is_some_and(|token| token.text.eq_ignore_ascii_case("AS"))
        {
            next += 1;
        }
        if let Some(alias) = significant.get(next)
            && (alias.opaque || alias.text.chars().all(word_char))
            && !is_keyword(alias.text)
        {
            aliases.push((unquote(alias.text), name, schema, scope.len()));
        }
    }
    let mut candidates = Vec::new();
    if let Some(qualifier) = qualifier {
        let binding = aliases
            .iter()
            .filter(|(alias, _, _, _)| identifier_matches(alias, &qualifier))
            .max_by_key(|(_, _, _, depth)| depth);
        for table in tables {
            if let Some((_, name, schema, _)) = binding {
                if !identifier_matches(&table.name, name)
                    || schema
                        .as_ref()
                        .is_some_and(|schema| !identifier_matches(&table.schema, schema))
                {
                    continue;
                }
                for column in &table.columns {
                    candidates.push((column.clone(), CompletionKind::Field));
                }
            } else if identifier_matches(&table.schema, &qualifier) {
                candidates.push((table.name.clone(), CompletionKind::Table));
            } else if identifier_matches(&table.name, &qualifier) {
                for column in &table.columns {
                    candidates.push((column.clone(), CompletionKind::Field));
                }
            }
        }
    } else {
        let names = tables
            .iter()
            .map(|table| table.name.clone())
            .collect::<Vec<_>>();
        let scoped_columns = tables
            .iter()
            .filter(|table| {
                aliases.iter().any(|(_, name, schema, _)| {
                    identifier_matches(&table.name, name)
                        && schema
                            .as_ref()
                            .is_none_or(|schema| identifier_matches(&table.schema, schema))
                })
            })
            .flat_map(|table| table.columns.iter().cloned())
            .collect::<Vec<_>>();
        candidates = completions(
            prefix,
            if aliases.is_empty() {
                fallback_columns
            } else {
                &scoped_columns
            },
            &names,
        );
        for table in tables {
            if !table.schema.is_empty() {
                candidates.push((table.schema.clone(), CompletionKind::Table));
            }
        }
    }
    candidates.retain(|(name, _)| {
        name.len() > prefix.len() && name.to_lowercase().starts_with(&prefix.to_lowercase())
    });
    candidates.sort_by_key(|(name, _)| name.to_lowercase());
    candidates.dedup_by(|a, b| {
        if engine == Engine::Postgres {
            a.0 == b.0
        } else {
            a.0.eq_ignore_ascii_case(&b.0)
        }
    });
    candidates.truncate(24);
    for (name, kind) in &mut candidates {
        if *kind != CompletionKind::Keyword
            && (is_keyword(name)
                || (engine == Engine::Postgres && name.chars().any(char::is_uppercase))
                || !name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
                || !name.chars().all(word_char))
        {
            *name = match engine {
                Engine::MySql => format!("`{}`", name.replace('`', "``")),
                Engine::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
                Engine::SqlServer => format!("[{}]", name.replace(']', "]]")),
            };
        }
    }
    Some((range, candidates))
}

/// Non-overlapping source spans for a collapsible outline of multiline
/// comments and parenthesized code; literal contents never become fold markers.
pub fn folding_ranges(sql: &str, engine: Engine) -> Result<Vec<Range<usize>>, String> {
    let tokens = editing_tokens(sql, engine)?;
    let mut stack = Vec::new();
    let mut ranges = Vec::new();
    for token in tokens {
        if token.comment && token.text.contains('\n') {
            ranges.push(token.range.clone());
        }
        if !token.opaque && !token.comment {
            if token.text == "(" {
                stack.push(token.range.start);
            }
            if token.text == ")"
                && let Some(start) = stack.pop()
                && sql[start..token.range.end].contains('\n')
            {
                ranges.push(start..token.range.end);
            }
        }
    }
    ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    let mut end = 0;
    ranges.retain(|range| {
        if range.start < end {
            false
        } else {
            end = range.end;
            true
        }
    });
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_formatting_preserves_literal_concatenation_newlines() {
        for (sql, expected) in [
            ("SELECT 'foo'\n'bar';", "SELECT 'foo'\n'bar';"),
            ("SELECT '中'\r\n'文';", "SELECT '中'\n'文';"),
            ("SELECT E'foo'\n'bar';", "SELECT E'foo'\n'bar';"),
            ("SELECT 'foo'\n'bar'\n'baz';", "SELECT 'foo'\n'bar'\n'baz';"),
        ] {
            for compact in [false, true] {
                let formatted = format_sql(sql, Engine::Postgres, compact).unwrap();
                assert_eq!(formatted, expected);
                assert_eq!(
                    format_sql(&formatted, Engine::Postgres, compact).unwrap(),
                    formatted
                );
            }
        }
        assert_eq!(
            format_sql("SELECT 'foo' 'bar';", Engine::Postgres, true).unwrap(),
            "SELECT 'foo' 'bar';"
        );
    }

    #[test]
    fn formatting_preserves_dialect_tokens_and_comment_boundaries() {
        for (engine, sql) in [
            (
                Engine::Postgres,
                "select E'\\\\🦀', $$a  b\nc$$, x::int from public.t -- tail\nwhere id > 1;",
            ),
            (
                Engine::SqlServer,
                "SELECT N'a  b', [中]]文], #temp.x FROM #temp;\nGO\nSELECT 2;",
            ),
            (
                Engine::MySql,
                "select `名``称`, 'a  b', 1--2 from t # comment\nwhere id=1;",
            ),
        ] {
            let tokens = editing_tokens(sql, engine)
                .unwrap()
                .iter()
                .map(|token| token.text.to_owned())
                .collect::<Vec<_>>();
            for compact in [true, false] {
                let formatted = format_sql(sql, engine, compact).unwrap();
                assert_eq!(
                    editing_tokens(&formatted, engine)
                        .unwrap()
                        .iter()
                        .map(|token| token.text.to_owned())
                        .collect::<Vec<_>>(),
                    tokens
                );
                assert_eq!(format_sql(&formatted, engine, compact).unwrap(), formatted);
            }
        }
        assert!(format_sql("SELECT 'a\\b'", Engine::MySql, false).is_err());
        assert!(format_sql("SELECT /* open", Engine::Postgres, false).is_err());
        for (engine, sql) in [
            (Engine::SqlServer, "SELECT 1;\n/* batch */ GO 2\nSELECT 3;"),
            (
                Engine::MySql,
                "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$\nDELIMITER ;",
            ),
        ] {
            let compact = format_sql(sql, engine, true).unwrap();
            assert_eq!(compact.lines().count(), sql.lines().count());
        }
    }

    #[test]
    fn semantic_completion_resolves_schema_aliases_and_scope() {
        let tables = vec![
            CompletionTable {
                schema: "public".into(),
                name: "users".into(),
                columns: vec!["user_id".into()],
            },
            CompletionTable {
                schema: "sales".into(),
                name: "orders".into(),
                columns: vec!["order_id".into()],
            },
        ];
        let check = |sql: &str, marker: &str| {
            let cursor = sql.find(marker).unwrap() + marker.len();
            semantic_completions(
                sql,
                byte_to_char_index(sql, cursor),
                Engine::Postgres,
                &tables,
                &[],
            )
            .unwrap()
            .1
        };
        assert_eq!(
            check("SELECT u. FROM public.users AS u", "u."),
            [("user_id".into(), CompletionKind::Field)]
        );
        assert_eq!(
            check("SELECT sales. FROM public.users", "sales."),
            [("orders".into(), CompletionKind::Table)]
        );
        assert_eq!(
            check("SELECT u.us FROM public.users u", "u.us"),
            [("user_id".into(), CompletionKind::Field)]
        );
        assert!(check("SELECT 1 FROM public.users u; SELECT u.", "u.").is_empty());
        assert!(check("SELECT (SELECT 1 FROM public.users u), (SELECT u.)", "u.").is_empty());
        assert_eq!(
            check(
                "SELECT (SELECT u. FROM sales.orders u) FROM public.users u",
                "u."
            ),
            [("order_id".into(), CompletionKind::Field)]
        );
        assert_eq!(
            check("SELECT (SELECT u.) FROM public.users u", "u."),
            [("user_id".into(), CompletionKind::Field)]
        );
        assert!(semantic_completions("SELECT 'u.'", 10, Engine::Postgres, &tables, &[]).is_none());
        assert_eq!(
            check("SELECT us FROM public.users", "SELECT us")
                .into_iter()
                .filter(|(_, kind)| *kind != CompletionKind::Keyword)
                .collect::<Vec<_>>(),
            [
                ("user_id".into(), CompletionKind::Field),
                ("users".into(), CompletionKind::Table)
            ]
        );
        let weird = vec![CompletionTable {
            schema: "public".into(),
            name: "users".into(),
            columns: vec!["odd name".into(), "select".into()],
        }];
        let sql = "SELECT u. FROM public.users u";
        assert_eq!(
            semantic_completions(sql, 9, Engine::Postgres, &weird, &[])
                .unwrap()
                .1,
            [
                ("\"odd name\"".into(), CompletionKind::Field),
                ("\"select\"".into(), CompletionKind::Field)
            ]
        );
        assert!(semantic_completions("x.", usize::MAX, Engine::Postgres, &tables, &[]).is_none());
        let cases = vec![CompletionTable {
            schema: "public".into(),
            name: "users".into(),
            columns: vec!["Name".into(), "name".into()],
        }];
        assert_eq!(
            semantic_completions(sql, 9, Engine::Postgres, &cases, &[])
                .unwrap()
                .1,
            [
                ("\"Name\"".into(), CompletionKind::Field),
                ("name".into(), CompletionKind::Field)
            ]
        );
        let sql = "SELECT u. FROM sales.orders u JOIN public.users AS \"U\" ON true";
        assert_eq!(
            semantic_completions(sql, 9, Engine::Postgres, &tables, &[])
                .unwrap()
                .1,
            [("order_id".into(), CompletionKind::Field)]
        );
    }

    #[test]
    fn folds_ignore_literal_parentheses_and_keep_utf8_ranges() {
        let sql = "SELECT '(\n)', (SELECT 中文\n FROM t); /* 中\n文 */";
        let ranges = folding_ranges(sql, Engine::Postgres).unwrap();
        assert_eq!(
            ranges
                .iter()
                .map(|range| &sql[range.clone()])
                .collect::<Vec<_>>(),
            ["(SELECT 中文\n FROM t)", "/* 中\n文 */"]
        );
    }

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
        let choices = completions(
            "中",
            &["中文列".into(), "中文列".into(), "其他".into()],
            &[],
        );
        assert_eq!(choices, [("中文列".into(), CompletionKind::Field)]);
        let mut text = sql.to_owned();
        let cursor = insert_completion(&mut text, range, &choices[0].0).unwrap();
        assert_eq!(text, "🦀 SELECT 中文列 FROM t");
        assert_eq!(cursor, "🦀 SELECT 中文列".chars().count());
        assert_eq!(
            completions("sel", &[], &[]),
            [("SELECT".into(), CompletionKind::Keyword)]
        );
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
    fn completion_offers_table_names_beside_keywords_and_columns() {
        let tables = [
            "users".to_string(),
            "user_roles".to_string(),
            "orders".to_string(),
        ];
        let choices = completions("use", &[], &tables);
        assert!(choices.contains(&("users".into(), CompletionKind::Table)));
        assert!(choices.contains(&("user_roles".into(), CompletionKind::Table)));
        assert!(!choices.iter().any(|(name, _)| name == "orders"));
        // A table sharing a name with a keyword or column is offered once,
        // as the higher-priority kind.
        let choices = completions("sel", &["seller".into()], &["select_log".into()]);
        assert_eq!(choices[0], ("SELECT".into(), CompletionKind::Keyword));
        let choices = completions("id", &["id_col".into()], &["identities".into()]);
        assert_eq!(
            choices,
            [
                ("id_col".into(), CompletionKind::Field),
                ("identities".into(), CompletionKind::Table),
            ]
        );
        assert!(completions("", &[], &tables).is_empty());
        assert_eq!(CompletionKind::Field.label(), "字段");
        assert_eq!(CompletionKind::Table.label(), "表");
        assert_eq!(CompletionKind::Keyword.label(), "关键字");
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
