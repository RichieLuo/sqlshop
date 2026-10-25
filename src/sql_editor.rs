//! Stateless SQL editor helpers. Byte ranges are always UTF-8 boundaries; egui
//! cursor positions, in contrast, count Unicode scalar values (not bytes).
mod format;
use format::*;
pub use format::{folding_ranges, format_sql};

mod completion;
pub use completion::{completion_word, completions, insert_completion, semantic_completions};

mod highlight;
pub use highlight::highlight_sql;
use highlight::*;

mod statements;
pub use statements::current_statement_range;
use statements::*;

use crate::model::Engine;
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

#[cfg(test)]
mod tests;
