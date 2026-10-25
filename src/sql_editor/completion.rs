use super::*;

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
