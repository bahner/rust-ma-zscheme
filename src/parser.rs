/// S-expression lexer and parser for the zscheme evaluator.
/// No external dependencies — pure Rust.

// ── AST ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SchemeExpr {
    /// `()` / `nil` — the empty list.
    #[allow(dead_code)]
    Nil,
    /// A quoted string literal: `"hello"`.
    Str(String),
    /// Any other token: symbol, number, keyword, path, actor target, …
    Atom(String),
    /// A parenthesised form: `(f a b …)`.
    List(Vec<SchemeExpr>),
}

// ── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct LexError(pub String);

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Lexer ──────────────────────────────────────────────────────────────────

/// Tokenise a Scheme source string.
///
/// # Errors
///
/// Returns `Err` if the input contains an unterminated string literal.
pub fn tokenize(input: &str) -> Result<Vec<String>, LexError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push("(".to_string());
            }
            ')' => {
                chars.next();
                tokens.push(")".to_string());
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut escaped = false;
                loop {
                    match chars.next() {
                        None => return Err(LexError("unterminated string literal".to_string())),
                        Some('\\') if !escaped => {
                            escaped = true;
                        }
                        Some('"') if !escaped => break,
                        Some(c) => {
                            if escaped {
                                match c {
                                    'n' => s.push('\n'),
                                    't' => s.push('\t'),
                                    'r' => s.push('\r'),
                                    '\\' => s.push('\\'),
                                    '"' => s.push('"'),
                                    other => {
                                        s.push('\\');
                                        s.push(other);
                                    }
                                }
                                escaped = false;
                            } else {
                                s.push(c);
                            }
                        }
                    }
                }
                tokens.push(format!("\"{s}\""));
            }
            ';' => while chars.next().is_some_and(|c| c != '\n') {},
            '|' => {
                chars.next();
                tokens.push("|".to_string());
            }
            '\'' => {
                chars.next();
                tokens.push("'QUOTE".to_string());
            }
            _ => {
                let mut atom = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' '
                        || c == '\t'
                        || c == '\n'
                        || c == '\r'
                        || c == '('
                        || c == ')'
                        || c == ';'
                        || c == '|'
                    {
                        break;
                    }
                    atom.push(c);
                    chars.next();
                }
                if !atom.is_empty() {
                    tokens.push(atom);
                }
            }
        }
    }

    Ok(tokens)
}

// ── Parser ─────────────────────────────────────────────────────────────────

/// Parse one S-expression from `tokens` starting at `pos`.
/// Returns `(expr, next_pos)` on success.
///
/// # Errors
///
/// Returns `Err` on unexpected end of input or mismatched parentheses.
pub fn parse_expr(tokens: &[String], pos: usize) -> Result<(SchemeExpr, usize), LexError> {
    if pos >= tokens.len() {
        return Err(LexError("unexpected end of input".to_string()));
    }

    let token = &tokens[pos];

    if token == "(" {
        let mut forms = Vec::new();
        let mut i = pos + 1;
        loop {
            if i >= tokens.len() {
                return Err(LexError("missing closing ')'".to_string()));
            }
            if tokens[i] == ")" {
                return Ok((SchemeExpr::List(forms), i + 1));
            }
            let (expr, next) = parse_expr(tokens, i)?;
            forms.push(expr);
            i = next;
        }
    } else if token == "'QUOTE" {
        let (inner, next_pos) = parse_expr(tokens, pos + 1)?;
        Ok((
            SchemeExpr::List(vec![SchemeExpr::Atom("quote".to_string()), inner]),
            next_pos,
        ))
    } else if token == ")" {
        Err(LexError("unexpected ')'".to_string()))
    } else if token.starts_with('"') && token.ends_with('"') && token.len() >= 2 {
        let inner = token[1..token.len() - 1].to_string();
        Ok((SchemeExpr::Str(inner), pos + 1))
    } else {
        Ok((SchemeExpr::Atom(token.clone()), pos + 1))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── tokenize ──

    /// Tokenise a simple arithmetic expression.
    ///
    /// ```
    /// # use ma_zscheme::parser::tokenize;
    /// let tokens = tokenize("(+ 1 2)").unwrap();
    /// assert_eq!(tokens, vec!["(", "+", "1", "2", ")"]);
    /// ```
    #[test]
    fn tokenize_basic_expr() {
        let tokens = tokenize("(+ 1 2)").unwrap();
        assert_eq!(tokens, vec!["(", "+", "1", "2", ")"]);
    }

    #[test]
    fn tokenize_string_literal() {
        let tokens = tokenize("\"hello world\"").unwrap();
        assert_eq!(tokens, vec!["\"hello world\""]);
    }

    #[test]
    fn tokenize_strips_comments() {
        let tokens = tokenize("; this is a comment\n(+ 1 2)").unwrap();
        assert_eq!(tokens, vec!["(", "+", "1", "2", ")"]);
    }

    #[test]
    fn tokenize_nested_list() {
        let tokens = tokenize("(if #t (+ 1 2) 0)").unwrap();
        assert_eq!(
            tokens,
            vec!["(", "if", "#t", "(", "+", "1", "2", ")", "0", ")"]
        );
    }

    #[test]
    fn tokenize_quote_shorthand() {
        let tokens = tokenize("'foo").unwrap();
        assert_eq!(tokens, vec!["'QUOTE", "foo"]);
    }

    #[test]
    fn tokenize_escape_sequences_in_string() {
        let tokens = tokenize("\"a\\nb\"").unwrap();
        assert_eq!(tokens, vec!["\"a\nb\""]);
    }

    #[test]
    fn tokenize_unterminated_string_is_err() {
        assert!(tokenize("\"oops").is_err());
    }

    // ── parse_expr ──

    /// Parse an atom.
    ///
    /// ```
    /// # use ma_zscheme::parser::{tokenize, parse_expr, SchemeExpr};
    /// let tokens = tokenize("foo").unwrap();
    /// let (expr, pos) = parse_expr(&tokens, 0).unwrap();
    /// assert!(matches!(expr, SchemeExpr::Atom(s) if s == "foo"));
    /// assert_eq!(pos, 1);
    /// ```
    #[test]
    fn parse_atom() {
        let tokens = tokenize("hello").unwrap();
        let (expr, pos) = parse_expr(&tokens, 0).unwrap();
        assert!(matches!(expr, SchemeExpr::Atom(s) if s == "hello"));
        assert_eq!(pos, 1);
    }

    #[test]
    fn parse_string_literal() {
        let tokens = tokenize("\"hi\"").unwrap();
        let (expr, _) = parse_expr(&tokens, 0).unwrap();
        assert!(matches!(expr, SchemeExpr::Str(s) if s == "hi"));
    }

    #[test]
    fn parse_list_length() {
        let tokens = tokenize("(+ 1 2)").unwrap();
        let (expr, _) = parse_expr(&tokens, 0).unwrap();
        if let SchemeExpr::List(forms) = expr {
            assert_eq!(forms.len(), 3);
        } else {
            panic!("expected List");
        }
    }

    #[test]
    fn parse_nested_list() {
        let tokens = tokenize("(if #t (+ 1 2) 0)").unwrap();
        let (expr, _) = parse_expr(&tokens, 0).unwrap();
        if let SchemeExpr::List(forms) = expr {
            assert_eq!(forms.len(), 4);
            assert!(matches!(&forms[2], SchemeExpr::List(_)));
        } else {
            panic!("expected List");
        }
    }

    #[test]
    fn parse_quote_shorthand_expands() {
        let tokens = tokenize("'x").unwrap();
        let (expr, _) = parse_expr(&tokens, 0).unwrap();
        if let SchemeExpr::List(forms) = expr {
            assert_eq!(forms.len(), 2);
            assert!(matches!(&forms[0], SchemeExpr::Atom(s) if s == "quote"));
        } else {
            panic!("expected quoted list");
        }
    }

    #[test]
    fn parse_unexpected_close_paren_is_err() {
        let tokens = vec![")".to_string()];
        assert!(parse_expr(&tokens, 0).is_err());
    }

    #[test]
    fn parse_empty_token_stream_is_err() {
        assert!(parse_expr(&[], 0).is_err());
    }
}
