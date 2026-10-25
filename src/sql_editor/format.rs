use super::*;

pub(super) fn editing_tokens(sql: &str, engine: Engine) -> Result<Vec<SqlLexeme<'_>>, String> {
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
