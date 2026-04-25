use crate::{lexer::Token, Span};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// Saw a schedule keyword at top level → next ident is a flow name.
    SeenSched,
    /// Saw `assert` → absorb an optional schedule keyword without opening a flow block.
    SeenAssert,
}

/// Convert the whitespace-aware token stream into a brace-delimited stream.
///
/// Flow bodies use indentation. Param interval bodies use explicit `{}`.
/// `Indent` tokens are consumed here and replaced with virtual `LBrace`/`RBrace`
/// tokens as needed.
///
/// `HardSpace` is kept only inside virtual (flow body) blocks where it acts as
/// the posting-amount separator. Outside those blocks it is structural whitespace
/// and is dropped.
pub fn layout(tokens: Vec<(Token<'_>, Span)>) -> Vec<(Token<'_>, Span)> {
    let mut out: Vec<(Token<'_>, Span)> = Vec::with_capacity(tokens.len());
    let mut indent_stack: Vec<usize> = vec![0];
    let mut awaiting_block = false;
    let mut explicit_brace_depth: usize = 0;
    // Tracks how many virtual (flow-body) blocks are open.
    let mut virtual_depth: usize = 0;
    let mut mode = Mode::Normal;

    for (token, span) in tokens {
        match &token {
            Token::HardSpace => {
                // Only meaningful inside a virtual (flow body) block.
                if virtual_depth > 0 && explicit_brace_depth == 0 {
                    out.push((token, span));
                }
                continue;
            }
            Token::Indent(n) => {
                let n = *n;
                if explicit_brace_depth > 0 {
                    // Inside a param `{ }` — indentation is irrelevant.
                    continue;
                }
                if awaiting_block {
                    // First line of a flow body: open a virtual block.
                    let start = span.start;
                    out.push((Token::LBrace, (start..start).into()));
                    indent_stack.push(n);
                    awaiting_block = false;
                    virtual_depth += 1;
                    continue;
                }
                // Close blocks we've de-indented out of.
                while indent_stack.len() > 1 && n <= indent_stack[indent_stack.len() - 2] {
                    let start = span.start;
                    out.push((Token::RBrace, (start..start).into()));
                    indent_stack.pop();
                    virtual_depth = virtual_depth.saturating_sub(1);
                }
                // Structural token — don't emit.
                continue;
            }
            Token::LBrace => {
                explicit_brace_depth += 1;
                mode = Mode::Normal;
                out.push((token, span));
            }
            Token::RBrace => {
                explicit_brace_depth = explicit_brace_depth.saturating_sub(1);
                out.push((token, span));
            }
            // `assert` → enter SeenAssert so the optional schedule doesn't open a flow block.
            Token::Ident("assert") => {
                mode = Mode::SeenAssert;
                out.push((token, span));
            }
            // Schedule keyword after `assert`: emit and return to Normal (no block queued).
            Token::Ident("daily" | "monthly" | "quarterly" | "yearly")
                if mode == Mode::SeenAssert =>
            {
                mode = Mode::Normal;
                out.push((token, span));
            }
            // `on` after `assert`: emit and stay in SeenAssert to absorb the following (date).
            Token::Ident("on") if mode == Mode::SeenAssert => {
                out.push((token, span));
            }
            // LParen / RParen / Date inside `assert on(date)`: absorb without leaving SeenAssert.
            Token::LParen | Token::RParen | Token::Date(_) if mode == Mode::SeenAssert => {
                out.push((token, span));
            }
            // Top-level schedule keywords for flows.
            Token::Ident("daily" | "monthly" | "quarterly" | "yearly") if mode == Mode::Normal => {
                mode = Mode::SeenSched;
                out.push((token, span));
            }
            Token::Ident("on") if mode == Mode::Normal => {
                mode = Mode::SeenSched;
                out.push((token, span));
            }
            Token::Ident(_) if mode == Mode::SeenSched => {
                // Flow name — next indented block is the body.
                mode = Mode::Normal;
                awaiting_block = true;
                out.push((token, span));
            }
            // Inside `on(date)` for a flow: LParen, RParen, Date stay in SeenSched.
            Token::LParen | Token::RParen | Token::Date(_) if mode == Mode::SeenSched => {
                out.push((token, span));
            }
            _ => {
                mode = Mode::Normal;
                out.push((token, span));
            }
        }
    }

    // Close any remaining virtual blocks at EOF.
    let eof_pos = out.last().map(|(_, s)| s.end).unwrap_or(0);
    while indent_stack.len() > 1 {
        out.push((Token::RBrace, (eof_pos..eof_pos).into()));
        indent_stack.pop();
        virtual_depth = virtual_depth.saturating_sub(1);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lexer;
    use chumsky::Parser;

    fn lex_and_layout(src: &str) -> Vec<Token<'_>> {
        let (toks, errs) = lexer().parse(src).into_output_errors();
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        layout(toks.unwrap()).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn flow_body_gets_braces() {
        let toks = lex_and_layout("monthly paycheck\n  Assets:Cash  100\n  Income:Gross\n");
        assert!(toks.contains(&Token::LBrace));
        assert!(toks.contains(&Token::RBrace));
    }

    #[test]
    fn hard_space_stripped_outside_flow() {
        // Alignment spaces in account declarations should be stripped.
        let toks = lex_and_layout("account Assets:Cash          = 5_000");
        assert!(!toks.contains(&Token::HardSpace));
    }

    #[test]
    fn hard_space_kept_inside_flow() {
        // The separator between posting account and amount must survive.
        let toks = lex_and_layout("monthly paycheck\n  Assets:Cash  100\n  Income:Gross\n");
        assert!(toks.contains(&Token::HardSpace));
    }

    #[test]
    fn param_brace_body_not_disturbed() {
        let src = "param salary_rate : usd/year {\n  from 2026-01-01 = 120_000\n}";
        let toks = lex_and_layout(src);
        let lbraces = toks.iter().filter(|t| matches!(t, Token::LBrace)).count();
        let rbraces = toks.iter().filter(|t| matches!(t, Token::RBrace)).count();
        assert_eq!(lbraces, 1, "expected 1 LBrace, got {lbraces}");
        assert_eq!(rbraces, 1, "expected 1 RBrace, got {rbraces}");
    }
}
