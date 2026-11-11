//! Lossless handling of static JavaScript strings, including serialized client
//! state. Code outside changed URL literals is copied verbatim, never formatted.

use std::ops::Range;

/// Visits literal strings and nested serialized strings, with a depth bound.
pub(super) fn strings(code: &str) -> Vec<String> {
    fn collect(code: &str, depth: usize, output: &mut Vec<String>) {
        for range in literals(code) {
            if let Some(value) = decode(&code[range]) {
                if depth < 3 && value.contains(['\"', '\'', '`']) {
                    collect(&value, depth + 1, output);
                }
                output.push(value);
            }
        }
    }
    let mut output = Vec::new();
    collect(code, 0, &mut output);
    output
}

/// Rewrites known URL values inside code or serialized hydration data. Unchanged
/// literals retain their original spelling and escaping.
pub(super) fn rewrite(code: &str, map: &impl Fn(&str, &str) -> Option<String>) -> String {
    fn visit(code: &str, map: &impl Fn(&str, &str) -> Option<String>, depth: usize) -> String {
        let mut output = String::with_capacity(code.len());
        let mut cursor = 0;
        for range in literals(code) {
            let Some(value) = decode(&code[range.clone()]) else {
                continue;
            };
            let replacement = map(&value, &code[..range.start]).or_else(|| {
                if depth >= 3 || !value.contains(['\"', '\'', '`']) {
                    return None;
                }
                let nested = visit(&value, map, depth + 1);
                (nested != value).then_some(nested)
            });
            if let Some(value) = replacement {
                output.push_str(&code[cursor..range.start]);
                // A literal originating inside <script> must not close its tag.
                output.push_str(
                    &serde_json::to_string(&value)
                        .expect("string serialization")
                        .replace('<', "\\u003c"),
                );
                cursor = range.end;
            }
        }
        output.push_str(&code[cursor..]);
        output
    }
    visit(code, map, 0)
}

/// Finds strings while leaving comments, regular expressions and interpolated
/// templates opaque. Computed dependency names require the original backend or
/// build system and are deliberately not guessed.
fn literals(code: &str) -> Vec<Range<usize>> {
    let bytes = code.as_bytes();
    let mut output = Vec::new();
    let mut i = 0;
    let mut expression_start = true;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b'/' if expression_start => {
                i += 1;
                let mut class = false;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'[' => {
                            class = true;
                            i += 1;
                        }
                        b']' => {
                            class = false;
                            i += 1;
                        }
                        b'/' if !class => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                expression_start = false;
            }
            quote @ (b'\"' | b'\'' | b'`') => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        output.push(start..i);
                        break;
                    }
                    i += 1;
                }
                expression_start = false;
            }
            byte if byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$'))
                {
                    i += 1;
                }
                expression_start = matches!(
                    &code[start..i],
                    "return"
                        | "throw"
                        | "case"
                        | "delete"
                        | "typeof"
                        | "void"
                        | "new"
                        | "yield"
                        | "await"
                        | "in"
                        | "of"
                );
            }
            byte if byte.is_ascii_whitespace() => i += 1,
            byte => {
                expression_start = matches!(
                    byte,
                    b'=' | b'('
                        | b'['
                        | b'{'
                        | b','
                        | b':'
                        | b';'
                        | b'!'
                        | b'?'
                        | b'&'
                        | b'|'
                        | b'+'
                        | b'-'
                        | b'*'
                        | b'%'
                        | b'~'
                        | b'<'
                        | b'>'
                );
                i += 1;
            }
        }
    }
    output
}

fn decode(literal: &str) -> Option<String> {
    let quote = *literal.as_bytes().first()?;
    let text = &literal[1..literal.len() - 1];
    if quote == b'`' && text.contains("${") {
        return None;
    }
    if quote == b'\"'
        && let Ok(value) = serde_json::from_str(literal)
    {
        return Some(value);
    }
    let mut output = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            output.push(c);
            continue;
        }
        output.push(match chars.next()? {
            '\n' => continue,
            '\r' => {
                if chars.as_str().starts_with('\n') {
                    chars.next();
                }
                continue;
            }
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\x08',
            'f' => '\x0c',
            'v' => '\x0b',
            '0' => '\0',
            kind @ ('x' | 'u') => {
                let width = if kind == 'x' { 2 } else { 4 };
                let hex: String = chars.by_ref().take(width).collect();
                if hex.len() != width {
                    return None;
                }
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            other => other,
        });
    }
    Some(output)
}
