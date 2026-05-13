//! Post-processing for log field values produced by [`std::fmt::Debug`].
//!
//! [`clean_debug_value`] strips two common sources of noise from the raw
//! `{:?}` output that [`tracing`] hands us:
//!
//! - **`Option` wrappers** — `Some(x)` becomes `x`; `None` is left as-is.
//! - **`serde_json::Value` Debug notation** — `Object {"k": String("v")}`
//!   becomes `{"k": "v"}`, stripping the per-variant type tags.
//!
//! Any string that doesn't match either pattern is returned unchanged via a
//! `Cow::Borrowed` reference, avoiding allocation.

use std::borrow::Cow;

/// Clean up a raw `{:?}` field value for human-readable console output.
///
/// Applies two passes in order:
/// 1. Strip `Some(…)` wrappers recursively (`None` is kept as-is).
/// 2. Rewrite `serde_json` Debug notation to compact JSON-like text.
///
/// Returns `Cow::Borrowed(s)` when nothing changed (no allocation).
pub(crate) fn clean_debug_value(s: &str) -> Cow<'_, str> {
    let stripped = strip_some_wrappers(s);
    if let Some(rewritten) = rewrite_json_debug(stripped) {
        Cow::Owned(rewritten)
    } else if stripped.len() != s.len() {
        Cow::Owned(stripped.to_owned())
    } else {
        Cow::Borrowed(s)
    }
}

// ── Some(…) stripping ─────────────────────────────────────────────────────────

/// Strip zero or more layers of `Some(…)`, returning the innermost content.
fn strip_some_wrappers(s: &str) -> &str {
    let mut current = s;
    while let Some(inner) = try_strip_some(current) {
        current = inner;
    }
    current
}

/// Remove one `Some(…)` layer from `s`, returning the inner slice, or `None`
/// if `s` is not exactly `Some(…)`.
fn try_strip_some(s: &str) -> Option<&str> {
    let inner = s.strip_prefix("Some(")?;
    let close = find_matching_close_paren(inner)?;
    // Only strip when the matching `)` is the final character — i.e. the
    // whole string is `Some(…)`, not something like `Some(x) trailing`.
    if close == inner.len() - 1 { Some(&inner[..close]) } else { None }
}

/// Return the byte index of the `)` that closes the already-opened `(` at the
/// start of `s`, scanning with depth tracking.
///
/// Properly handles:
/// - Nested parentheses: `foo(bar)` — depth tracks all `(`/`)` pairs.
/// - Rust string literals: `"…"` including `\"` escapes — parens inside
///   strings don't affect depth, preventing false matches like `Some(")")`.
fn find_matching_close_paren(s: &str) -> Option<usize> {
    let mut depth: usize = 1;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                // Skip the string literal so parens inside don't affect depth.
                loop {
                    match chars.next() {
                        Some((_, '\\')) => {
                            chars.next(); // skip the escaped character
                        }
                        Some((_, '"')) => break,
                        None => return None,
                        _ => {}
                    }
                }
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// ── serde_json Debug rewriter ─────────────────────────────────────────────────

/// Attempt to rewrite a `serde_json` `{:?}` value into compact JSON-like text.
///
/// Handles `Object`, `Array`, `String`, `Number`, `Bool`, and `Null`.
/// Returns `None` if the input doesn't match the `serde_json` Debug grammar,
/// leaving the caller to pass the original string through unchanged.
fn rewrite_json_debug(s: &str) -> Option<String> {
    let mut p = JsonDebugParser::new(s);
    let mut out = String::with_capacity(s.len());
    if p.parse_value(&mut out) && p.pos == s.len() { Some(out) } else { None }
}

struct JsonDebugParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> JsonDebugParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.remaining().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.advance(1);
        }
    }

    fn parse_value(&mut self, out: &mut String) -> bool {
        let rem = self.remaining();
        if rem.starts_with("Object {") {
            self.parse_object(out)
        } else if rem.starts_with("Array [") {
            self.parse_array(out)
        } else if rem.starts_with("String(") {
            self.advance("String(".len());
            if !self.parse_rust_string(out) {
                return false;
            }
            self.eat(")")
        } else if rem.starts_with("Number(") {
            self.advance("Number(".len());
            let start = self.pos;
            while self.peek().is_some_and(|c| c != ')') {
                let Some(c) = self.peek() else { return false };
                self.advance(c.len_utf8());
            }
            let num = &self.input[start..self.pos];
            if num.is_empty() {
                return false;
            }
            out.push_str(num);
            self.eat(")")
        } else if rem.starts_with("Bool(true)") {
            self.advance("Bool(true)".len());
            out.push_str("true");
            true
        } else if rem.starts_with("Bool(false)") {
            self.advance("Bool(false)".len());
            out.push_str("false");
            true
        } else if rem.starts_with("Null") {
            self.advance("Null".len());
            out.push_str("null");
            true
        } else {
            false
        }
    }

    fn parse_object(&mut self, out: &mut String) -> bool {
        self.advance("Object ".len());
        if !self.eat("{") {
            return false;
        }
        out.push('{');
        self.skip_ws();

        if self.peek() == Some('}') {
            self.advance(1);
            out.push('}');
            return true;
        }

        let mut first = true;
        loop {
            if !first {
                if !self.eat(", ") {
                    return false;
                }
                out.push_str(", ");
            }
            first = false;

            if !self.parse_rust_string(out) {
                return false;
            }
            if !self.eat(": ") {
                return false;
            }
            out.push_str(": ");
            if !self.parse_value(out) {
                return false;
            }

            self.skip_ws();
            if self.peek() == Some('}') {
                self.advance(1);
                out.push('}');
                break;
            }
        }
        true
    }

    fn parse_array(&mut self, out: &mut String) -> bool {
        self.advance("Array ".len());
        if !self.eat("[") {
            return false;
        }
        out.push('[');
        self.skip_ws();

        if self.peek() == Some(']') {
            self.advance(1);
            out.push(']');
            return true;
        }

        let mut first = true;
        loop {
            if !first {
                if !self.eat(", ") {
                    return false;
                }
                out.push_str(", ");
            }
            first = false;

            if !self.parse_value(out) {
                return false;
            }

            self.skip_ws();
            if self.peek() == Some(']') {
                self.advance(1);
                out.push(']');
                break;
            }
        }
        true
    }

    /// Parse a Rust `{:?}` string literal (`"…"` with `\"` and `\\` escapes),
    /// writing the content verbatim (including surrounding quotes) into `out`.
    fn parse_rust_string(&mut self, out: &mut String) -> bool {
        if self.peek() != Some('"') {
            return false;
        }
        out.push('"');
        self.advance(1);
        loop {
            match self.peek() {
                None => return false,
                Some('"') => {
                    out.push('"');
                    self.advance(1);
                    return true;
                }
                Some('\\') => {
                    out.push('\\');
                    self.advance(1);
                    match self.peek() {
                        Some(c) => {
                            out.push(c);
                            self.advance(c.len_utf8());
                        }
                        None => return false,
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.advance(c.len_utf8());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(s: &str) -> String {
        clean_debug_value(s).into_owned()
    }

    // ── Some(…) stripping ─────────────────────────────────────────────────

    #[test]
    fn some_integer_is_unwrapped() {
        assert_eq!(clean("Some(3)"), "3");
    }

    #[test]
    fn some_false_is_unwrapped() {
        assert_eq!(clean("Some(false)"), "false");
    }

    #[test]
    fn some_true_is_unwrapped() {
        assert_eq!(clean("Some(true)"), "true");
    }

    #[test]
    fn some_nested_is_unwrapped_fully() {
        assert_eq!(clean("Some(Some(3))"), "3");
    }

    #[test]
    fn some_triple_nested_is_unwrapped_fully() {
        assert_eq!(clean("Some(Some(Some(42)))"), "42");
    }

    #[test]
    fn none_is_kept_as_is() {
        assert_eq!(clean("None"), "None");
    }

    #[test]
    fn some_with_string_inner_is_unwrapped() {
        assert_eq!(clean(r#"Some("hello")"#), r#""hello""#);
    }

    #[test]
    fn some_with_paren_in_string_is_handled() {
        // The `(` inside the string literal must not confuse the depth counter.
        assert_eq!(clean(r#"Some("a(b)")"#), r#""a(b)""#);
    }

    #[test]
    fn some_with_trailing_text_is_not_stripped() {
        // `Some(x) extra` is not a clean `Some(…)` — leave it alone.
        assert_eq!(clean("Some(3) extra"), "Some(3) extra");
    }

    // ── serde_json Debug rewriting ────────────────────────────────────────

    #[test]
    fn json_string_variant_is_unwrapped() {
        assert_eq!(clean(r#"String("hello")"#), r#""hello""#);
    }

    #[test]
    fn json_number_variant_is_unwrapped() {
        assert_eq!(clean("Number(42)"), "42");
    }

    #[test]
    fn json_number_negative() {
        assert_eq!(clean("Number(-7)"), "-7");
    }

    #[test]
    fn json_number_float() {
        assert_eq!(clean("Number(3.14)"), "3.14");
    }

    #[test]
    fn json_bool_true_is_unwrapped() {
        assert_eq!(clean("Bool(true)"), "true");
    }

    #[test]
    fn json_bool_false_is_unwrapped() {
        assert_eq!(clean("Bool(false)"), "false");
    }

    #[test]
    fn json_null_is_lowercased() {
        assert_eq!(clean("Null"), "null");
    }

    #[test]
    fn json_empty_object() {
        assert_eq!(clean("Object {}"), "{}");
    }

    #[test]
    fn json_object_single_string_field() {
        assert_eq!(clean(r#"Object {"event": String("SIGNED_IN")}"#), r#"{"event": "SIGNED_IN"}"#);
    }

    #[test]
    fn json_object_mixed_fields() {
        assert_eq!(
            clean(r#"Object {"event": String("SIGNED_IN"), "expiresAt": Number(1778671749)}"#),
            r#"{"event": "SIGNED_IN", "expiresAt": 1778671749}"#
        );
    }

    #[test]
    fn json_object_three_fields() {
        assert_eq!(
            clean(
                r#"Object {"event": String("SIGNED_IN"), "expiresAt": Number(1778671749), "userId": String("c6057bbf")}"#
            ),
            r#"{"event": "SIGNED_IN", "expiresAt": 1778671749, "userId": "c6057bbf"}"#
        );
    }

    #[test]
    fn json_empty_array() {
        assert_eq!(clean("Array []"), "[]");
    }

    #[test]
    fn json_array_of_strings() {
        assert_eq!(clean(r#"Array [String("a"), String("b")]"#), r#"["a", "b"]"#);
    }

    #[test]
    fn json_array_mixed() {
        assert_eq!(
            clean(r#"Array [String("x"), Number(1), Bool(true), Null]"#),
            r#"["x", 1, true, null]"#
        );
    }

    #[test]
    fn json_nested_object_in_array() {
        assert_eq!(clean(r#"Array [Object {"k": String("v")}]"#), r#"[{"k": "v"}]"#);
    }

    // ── Composed: Some wrapping a serde_json value ────────────────────────

    #[test]
    fn some_wrapping_json_object_is_fully_cleaned() {
        assert_eq!(clean(r#"Some(Object {"k": String("v")})"#), r#"{"k": "v"}"#);
    }

    #[test]
    fn some_wrapping_json_string_is_fully_cleaned() {
        assert_eq!(clean(r#"Some(String("hello"))"#), r#""hello""#);
    }

    // ── Passthrough ───────────────────────────────────────────────────────

    #[test]
    fn plain_integer_is_passed_through() {
        assert_eq!(clean("42"), "42");
    }

    #[test]
    fn plain_bool_is_passed_through() {
        assert_eq!(clean("true"), "true");
    }

    #[test]
    fn plain_string_is_passed_through() {
        assert_eq!(clean("hello"), "hello");
    }

    #[test]
    fn enum_variant_with_parens_is_passed_through() {
        // Not a serde_json pattern — leave as-is.
        assert_eq!(clean("MyEnum(stuff)"), "MyEnum(stuff)");
    }

    #[test]
    fn empty_string_is_passed_through() {
        assert_eq!(clean(""), "");
    }

    #[test]
    fn unchanged_values_borrow_without_allocating() {
        let s = "plain value";
        assert!(matches!(clean_debug_value(s), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn some_unwrap_produces_owned() {
        let s = "Some(3)";
        assert!(matches!(clean_debug_value(s), std::borrow::Cow::Owned(_)));
    }
}
