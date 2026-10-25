use super::*;

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

pub(super) fn statement_comment_end(
    sql: &str,
    mut offset: usize,
    engine: Engine,
) -> Result<usize, String> {
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

pub(super) fn statement_dollar_delimiter(text: &str) -> Option<&str> {
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

pub(super) fn statement_quote_end(
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

pub(super) fn reject_statement_body(words: &[String], engine: Engine) -> Result<(), String> {
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
