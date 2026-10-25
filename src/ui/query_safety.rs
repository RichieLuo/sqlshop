pub(super) fn sql_needs_confirmation(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut first = true;
    let mut word = "";
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end);
            }
            b'#' => i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let Some(end) = sql[i + 2..].find("*/") else {
                    return true;
                };
                i += end + 4;
            }
            b';' if first => i += 1,
            b';' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b' ' | b'\t' | b'\r' | b'\n' | b';' => i += 1,
                        b'-' if bytes.get(i + 1) == Some(&b'-') => {
                            i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end);
                        }
                        b'#' => i = sql[i..].find('\n').map_or(bytes.len(), |end| i + end),
                        b'/' if bytes.get(i + 1) == Some(&b'*') => {
                            let Some(end) = sql[i + 2..].find("*/") else {
                                return true;
                            };
                            i += end + 4;
                        }
                        _ => return true,
                    }
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' if first => {
                let start = i;
                while bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
                    i += 1;
                }
                let first_word = &sql[start..i];
                if !first_word.eq_ignore_ascii_case("SELECT")
                    && !first_word.eq_ignore_ascii_case("SHOW")
                    && !first_word.eq_ignore_ascii_case("DESCRIBE")
                {
                    return true;
                }
                if bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    return true;
                }
                word = first_word;
                first = false;
            }
            b'\'' | b'"' | b'`' | b'[' if !first => {
                let close = if bytes[i] == b'[' { b']' } else { bytes[i] };
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && close != b']' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == close {
                        i += 1;
                        if bytes.get(i) == Some(&close) {
                            i += 1;
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return true;
                }
            }
            b'$' if !first => {
                let start = i;
                i += 1;
                while bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'$') {
                    let delimiter = &sql[start..=i];
                    i += 1;
                    let Some(end) = sql[i..].find(delimiter) else {
                        return true;
                    };
                    i += end + delimiter.len();
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' if !first => {
                let start = i;
                while bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
                    i += 1;
                }
                let token = &sql[start..i];
                if word.eq_ignore_ascii_case("SELECT") && token.eq_ignore_ascii_case("INTO") {
                    return true;
                }
            }
            _ if first => return true,
            _ => i += 1,
        }
    }
    first
}
