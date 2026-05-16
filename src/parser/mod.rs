mod schedule;

use crate::ast::{
    AggKind, BinOp, Decl, Expr, Interval, ParamBody, Path, Posting, PostingAmount, Program,
    ScheduleRef, SpannedExpr,
};
use crate::errors::Diagnostic;
use crate::lexer::Token;
use crate::Span;

pub struct Parser<'src> {
    tokens: Vec<(Token<'src>, Span)>,
    current: usize,
    errors: Vec<Diagnostic>,
    last_span: Span,
}

impl<'src> Parser<'src> {
    pub fn new(tokens: Vec<(Token<'src>, Span)>) -> Self {
        let last_span = tokens.first().map(|(_, s)| *s).unwrap_or(Span::new(0, 0));
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
            last_span,
        }
    }

    pub fn parse(&mut self) -> Result<Program, Vec<Diagnostic>> {
        let program = self.parse_program();
        if self.errors.is_empty() {
            Ok(program)
        } else {
            Err(std::mem::take(&mut self.errors))
        }
    }

    fn peek(&self) -> &Token<'src> {
        self.tokens
            .get(self.current)
            .map(|(t, _)| t)
            .unwrap_or(&Token::EOF)
    }

    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.current)
            .map(|(_, s)| *s)
            .unwrap_or(self.last_span)
    }

    fn peek_next(&self) -> &Token<'src> {
        self.tokens
            .get(self.current + 1)
            .map(|(t, _)| t)
            .unwrap_or(&Token::EOF)
    }

    fn advance(&mut self) -> (Token<'src>, Span) {
        if self.current < self.tokens.len() {
            let (t, s) = self.tokens[self.current].clone();
            self.last_span = s;
            self.current += 1;
            (t, s)
        } else {
            (Token::EOF, self.last_span)
        }
    }

    fn eat(&mut self, expected: &Token) -> Option<Span> {
        if self.peek() == expected {
            let (_, s) = self.advance();
            Some(s)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token<'src>) -> Option<Span> {
        if let Some(s) = self.eat(expected) {
            return Some(s);
        }
        let span = self.peek_span();
        self.errors
            .push(Diagnostic::new(span, format!("expected `{expected}`")));
        None
    }

    fn eat_ident(&mut self) -> Option<(&'src str, Span)> {
        if let Token::Ident(s) = self.peek() {
            let s = *s;
            let (_, span) = self.advance();
            Some((s, span))
        } else {
            None
        }
    }

    fn eat_ident_ci(&mut self, word: &str) -> Option<Span> {
        if let Token::Ident(s) = self.peek()
            && s.eq_ignore_ascii_case(word) {
                let (_, span) = self.advance();
                return Some(span);
        }
        None
    }

    fn save(&self) -> (usize, usize) {
        (self.current, self.errors.len())
    }

    fn restore(&mut self, (pos, err_len): (usize, usize)) {
        self.current = pos;
        self.errors.truncate(err_len);
    }

    // item (, item)* [,? and item]
    fn parse_comma_list<T, F>(&mut self, mut parse_item: F) -> Vec<T>
    where
        F: FnMut(&mut Self) -> Option<T>,
    {
        let Some(first) = parse_item(self) else {
            return Vec::new();
        };
        let mut items = vec![first];
        loop {
            if self.eat(&Token::Comma).is_some() {
                let _ = self.eat_ident_ci("and");
                if let Some(item) = parse_item(self) {
                    items.push(item);
                } else {
                    break;
                }
            } else if self.eat_ident_ci("and").is_some() {
                if let Some(item) = parse_item(self) {
                    items.push(item);
                }
                break;
            } else {
                break;
            }
        }
        items
    }

    // ---- program / declarations ----

    fn parse_program(&mut self) -> Program {
        let mut decls = Vec::new();
        while *self.peek() != Token::EOF {
            let start = self.peek_span();
            let err_count = self.errors.len();
            if let Some(decl) = self.parse_decl() {
                let span = Span::new(start.start, self.last_span.end);
                decls.push((decl, span));
            } else if *self.peek() != Token::EOF {
                // Keep only the first error from this failed declaration, then
                // skip to the next declaration boundary to avoid cascades.
                self.errors.truncate(err_count + 1);
                self.synchronize();
            }
        }
        Program { decls }
    }

    // Skip tokens until we reach something that can only start a top-level
    // declaration. Identifiers and dates are excluded because they appear freely
    // inside expressions; only the dedicated keyword tokens are reliable anchors.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                Token::EOF | Token::Account | Token::Assert | Token::Entry | Token::Param | Token::Schedule => {
                    return
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn parse_decl(&mut self) -> Option<Decl> {
        match self.peek() {
            Token::Assert => {
                self.advance();
                return self.parse_assert_decl();
            }
            Token::Entry => {
                self.advance();
                return self.parse_entry_decl();
            }
            Token::Param => {
                self.advance();
                return self.parse_param_decl();
            }
            Token::Schedule => {
                self.advance();
                return self.parse_schedule_decl();
            }
            Token::Account => {
                self.advance();
                return self.parse_account_decl();
            }
            Token::EOF => return None,
            _ => {}
        }
        let span = self.peek_span();
        self.errors
            .push(Diagnostic::new(span, "expected declaration"));
        None
    }

    fn parse_account_decl(&mut self) -> Option<Decl> {
        let name = self.parse_colon_path()?;
        let init = if self.eat(&Token::Eq).is_some() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Some(Decl::Account { name, init })
    }

    fn parse_schedule_decl(&mut self) -> Option<Decl> {
        let (name, _) = self.eat_ident()?;
        self.expect(&Token::Eq)?;
        let schedule = self.parse_schedule_literal()?;
        Some(Decl::Schedule {
            name: name.to_string(),
            schedule,
        })
    }

    fn parse_param_decl(&mut self) -> Option<Decl> {
        let (name, _) = self.eat_ident()?;
        let unit = if self.eat(&Token::Colon).is_some() {
            Some(self.parse_unit()?)
        } else {
            None
        };
        let body = if self.eat(&Token::Eq).is_some() {
            ParamBody::Const(self.parse_expr()?)
        } else if self.eat(&Token::LBrace).is_some() {
            let mut intervals = Vec::new();
            while *self.peek() != Token::RBrace && *self.peek() != Token::EOF {
                if let Some(iv) = self.parse_interval() {
                    intervals.push(iv);
                } else {
                    break;
                }
            }
            self.expect(&Token::RBrace)?;
            ParamBody::Schedule(intervals)
        } else {
            let span = self.peek_span();
            self.errors.push(Diagnostic::new(
                span,
                "expected `=` or `{` in param declaration",
            ));
            return None;
        };
        Some(Decl::Param {
            name: name.to_string(),
            unit,
            body,
        })
    }

    fn parse_assert_decl(&mut self) -> Option<Decl> {
        let schedule = self.try_parse_schedule_ref();
        self.expect(&Token::That)?;
        let asserted = self.parse_expr()?;
        Some(Decl::Assert { schedule, asserted })
    }

    fn parse_entry_decl(&mut self) -> Option<Decl> {
        let schedule = self.parse_schedule_ref_required()?;
        let label = if let Token::Str(s) = self.peek() {
            let s = s.to_string();
            self.advance();
            s
        } else {
            let span = self.peek_span();
            self.errors
                .push(Diagnostic::new(span, "expected string label"));
            return None;
        };
        self.expect(&Token::LBrace)?;
        let mut postings = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::EOF {
            if let Some(p) = self.parse_posting() {
                postings.push(p);
            } else {
                break;
            }
        }
        self.expect(&Token::RBrace)?;
        let alias = if self.eat_ident_ci("as").is_some() {
            self.eat_ident().map(|(s, _)| s.to_string())
        } else {
            None
        };
        Some(Decl::Entry {
            label,
            alias,
            schedule,
            postings,
        })
    }

    fn parse_schedule_ref_required(&mut self) -> Option<ScheduleRef> {
        if let Some(sr) = self.try_parse_schedule_ref() {
            return Some(sr);
        }
        let span = self.peek_span();
        self.errors.push(Diagnostic::new(span, "expected schedule"));
        None
    }

    fn try_parse_schedule_ref(&mut self) -> Option<ScheduleRef> {
        let cp = self.save();
        if let Some(sched) = self.parse_schedule_literal() {
            return Some(ScheduleRef::Literal(sched));
        }
        self.restore(cp);

        let (name, _) = self.eat_ident()?;
        Some(ScheduleRef::Named(name.to_string()))
    }

    fn parse_interval(&mut self) -> Option<Interval> {
        self.eat_ident_ci("from")?;
        let from = self.parse_date()?;
        let to = if self.eat_ident_ci("to").is_some() {
            Some(self.parse_date()?)
        } else {
            None
        };
        self.expect(&Token::Eq)?;
        let value = self.parse_expr()?;
        Some(Interval { from, to, value })
    }

    fn parse_unit(&mut self) -> Option<String> {
        let first = match self.peek() {
            Token::Ident(s) => {
                let s = s.to_string();
                self.advance();
                s
            }
            Token::Percent => {
                self.advance();
                "%".to_string()
            }
            _ => {
                let span = self.peek_span();
                self.errors.push(Diagnostic::new(span, "expected unit"));
                return None;
            }
        };
        if self.eat(&Token::Slash).is_some() {
            let second = match self.peek() {
                Token::Ident(s) => {
                    let s = s.to_string();
                    self.advance();
                    s
                }
                Token::Percent => {
                    self.advance();
                    "%".to_string()
                }
                _ => {
                    let span = self.peek_span();
                    self.errors
                        .push(Diagnostic::new(span, "expected unit after `/`"));
                    return None;
                }
            };
            Some(format!("{first}/{second}"))
        } else {
            Some(first)
        }
    }

    fn parse_posting(&mut self) -> Option<Posting> {
        if !matches!(self.peek(), Token::Ident(_)) {
            return None;
        }
        let account = self.parse_colon_path()?;
        let amount = if self.eat(&Token::Eq).is_some() {
            if self.eat_ident_ci("all").is_some() {
                Some(PostingAmount::All)
            } else {
                Some(PostingAmount::Expr(self.parse_expr()?))
            }
        } else {
            None
        };
        let leg_name = if self.eat_ident_ci("as").is_some() {
            self.eat_ident().map(|(s, _)| s.to_string())
        } else {
            None
        };
        Some(Posting {
            account,
            amount,
            leg_name,
        })
    }

    // ---- expressions ----

    fn parse_expr(&mut self) -> Option<SpannedExpr> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Option<SpannedExpr> {
        let mut left = self.parse_sum()?;
        loop {
            let op = match self.peek() {
                Token::LtEq => BinOp::LtEq,
                Token::GtEq => BinOp::GtEq,
                Token::EqEq => BinOp::Eq,
                Token::Lt => BinOp::Lt,
                Token::Gt => BinOp::Gt,
                _ => break,
            };
            self.advance();
            let right = self.parse_sum()?;
            let span = Span::new(left.1.start, right.1.end);
            left = (Box::new(Expr::Bin(left, op, right)), span);
        }
        Some(left)
    }

    fn parse_sum(&mut self) -> Option<SpannedExpr> {
        let mut left = self.parse_product()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_product()?;
            let span = Span::new(left.1.start, right.1.end);
            left = (Box::new(Expr::Bin(left, op, right)), span);
        }
        Some(left)
    }

    fn parse_product(&mut self) -> Option<SpannedExpr> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            let span = Span::new(left.1.start, right.1.end);
            left = (Box::new(Expr::Bin(left, op, right)), span);
        }
        Some(left)
    }

    fn parse_unary(&mut self) -> Option<SpannedExpr> {
        if let Some(minus_span) = self.eat(&Token::Minus) {
            let inner = self.parse_unary()?;
            let span = Span::new(minus_span.start, inner.1.end);
            Some((Box::new(Expr::Neg(inner)), span))
        } else {
            self.parse_atom()
        }
    }

    fn parse_atom(&mut self) -> Option<SpannedExpr> {
        let start = self.peek_span();

        if self.eat_ident_ci("if").is_some() {
            let cond = self.parse_expr()?;
            if self.eat_ident_ci("then").is_none() {
                self.errors
                    .push(Diagnostic::new(self.peek_span(), "expected `then`"));
                return None;
            }
            let then = self.parse_expr()?;
            if self.eat_ident_ci("else").is_none() {
                self.errors
                    .push(Diagnostic::new(self.peek_span(), "expected `else`"));
                return None;
            }
            let else_ = self.parse_expr()?;
            let span = Span::new(start.start, self.last_span.end);
            return Some((Box::new(Expr::If { cond, then, else_ }), span));
        }

        if let Some(s) = self.eat(&Token::True) {
            return Some((Box::new(Expr::Bool(true)), s));
        }
        if let Some(s) = self.eat(&Token::False) {
            return Some((Box::new(Expr::Bool(false)), s));
        }

        if let Token::Float(f) = self.peek() {
            let f = *f;
            let (_, s) = self.advance();
            return Some((Box::new(Expr::Num(f)), s));
        }

        if matches!(self.peek(), Token::Ident(_)) {
            if *self.peek_next() == Token::LParen {
                return self.parse_call();
            }
            if *self.peek_next() == Token::Period {
                return self.parse_calc();
            }
            return self.parse_colon_path_expr();
        }

        if self.eat(&Token::LParen).is_some() {
            let inner = self.parse_expr()?;
            self.expect(&Token::RParen);
            return Some(inner);
        }

        let span = self.peek_span();
        self.errors
            .push(Diagnostic::new(span, "expected expression"));
        None
    }

    fn parse_call(&mut self) -> Option<SpannedExpr> {
        let start = self.peek_span();
        let (name, _) = self.eat_ident()?;
        self.eat(&Token::LParen);
        let mut args = Vec::new();
        while *self.peek() != Token::RParen && *self.peek() != Token::EOF {
            if let Some(arg) = self.parse_expr() {
                args.push(arg);
            } else {
                break;
            }
            if self.eat(&Token::Comma).is_none() {
                break;
            }
        }
        self.expect(&Token::RParen);
        let span = Span::new(start.start, self.last_span.end);
        Some((Box::new(Expr::Call(name.to_string(), args)), span))
    }

    fn parse_calc(&mut self) -> Option<SpannedExpr> {
        let start = self.peek_span();
        let (first, _) = self.eat_ident()?;
        self.eat(&Token::Period);

        let cp = self.save();
        if let Some((second, _)) = self.eat_ident() {
            if self.eat(&Token::Period).is_some() {
                if let Some(kind) = self.try_eat_agg_kind() {
                    let span = Span::new(start.start, self.last_span.end);
                    return Some((
                        Box::new(Expr::ParamAgg(
                            Some(first.to_string()),
                            second.to_string(),
                            kind,
                        )),
                        span,
                    ));
                }
            }
        }
        self.restore(cp);

        if let Some(kind) = self.try_eat_agg_kind() {
            let span = Span::new(start.start, self.last_span.end);
            return Some((
                Box::new(Expr::ParamAgg(None, first.to_string(), kind)),
                span,
            ));
        }

        let span = self.peek_span();
        self.errors
            .push(Diagnostic::new(span, "expected ytd, qtd, or mtd"));
        None
    }

    fn try_eat_agg_kind(&mut self) -> Option<AggKind> {
        if let Token::Ident(s) = self.peek() {
            let kind = match s.to_lowercase().as_str() {
                "ytd" => AggKind::Ytd,
                "qtd" => AggKind::Qtd,
                "mtd" => AggKind::Mtd,
                _ => return None,
            };
            self.advance();
            Some(kind)
        } else {
            None
        }
    }

    fn parse_colon_path(&mut self) -> Option<Path> {
        let (first, _) = self.eat_ident()?;
        let mut parts = vec![first.to_string()];
        while self.eat(&Token::Colon).is_some() {
            if let Some((part, _)) = self.eat_ident() {
                parts.push(part.to_string());
            } else {
                let span = self.peek_span();
                self.errors
                    .push(Diagnostic::new(span, "expected identifier after `:`"));
                break;
            }
        }
        Some(Path(parts))
    }

    fn parse_colon_path_expr(&mut self) -> Option<SpannedExpr> {
        let start = self.peek_span();
        let path = self.parse_colon_path()?;
        let span = Span::new(start.start, self.last_span.end);
        Some((Box::new(Expr::Ref(path)), span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ast::schedule::Schedule, lexer::Lexer};
    use chrono::NaiveDate;

    fn parse_prog(src: &str) -> Program {
        let Ok(tokens) = Lexer::new(src).lex() else {
            panic!("lexer errored")
        };
        match Parser::new(tokens).parse() {
            Ok(prog) => prog,
            Err(errs) => panic!("parse errs: {errs:?}"),
        }
    }

    #[test]
    fn parses_account_with_init() {
        let prog = parse_prog("account Liabilities:Loan = -3_000_000");
        assert_eq!(prog.decls.len(), 1);
        match &prog.decls[0].0 {
            Decl::Account { name, init } => {
                assert_eq!(name.join(), "Liabilities:Loan");
                assert!(matches!(init, Some((e, _)) if matches!(e.as_ref(), Expr::Neg(_))));
            }
            _ => panic!("expected account"),
        }
    }

    #[test]
    fn parses_account_no_init() {
        let prog = parse_prog("account Assets:Cash");
        match &prog.decls[0].0 {
            Decl::Account { name, init } => {
                assert_eq!(name.join(), "Assets:Cash");
                assert!(init.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_param_const() {
        let prog = parse_prog("param interest_rate = 0.05");
        match &prog.decls[0].0 {
            Decl::Param { name, unit, body } => {
                assert_eq!(name, "interest_rate");
                assert!(unit.is_none());
                assert!(matches!(body, ParamBody::Const(_)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_param_schedule() {
        let prog = parse_prog(
            "param salary_rate : usd/year { from 2026-01-01 to 2026-04-01 = 215_000\nfrom 2026-04-01 = 220_000 }",
        );
        match &prog.decls[0].0 {
            Decl::Param { name, unit, body } => {
                assert_eq!(name, "salary_rate");
                assert_eq!(unit.as_deref(), Some("usd/year"));
                let ParamBody::Schedule(intervals) = body else {
                    panic!("Not a schedule body")
                };
                assert_eq!(intervals.len(), 2);
                assert_eq!(
                    intervals[0].from,
                    NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
                );
                assert!(intervals[1].to.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_flow_with_postings() {
        let prog = parse_prog(
            "entry monthly \"paycheck\" { Assets:Cash = salary_rate / 12\nIncome:Gross }",
        );
        match &prog.decls[0].0 {
            Decl::Entry {
                label,
                alias,
                schedule,
                postings,
            } => {
                assert_eq!(label, "paycheck");
                assert!(alias.is_none());
                assert!(matches!(
                    schedule,
                    ScheduleRef::Literal(Schedule::Periodic(_))
                ));
                assert_eq!(postings.len(), 2);
                assert_eq!(postings[0].account.join(), "Assets:Cash");
                assert!(postings[0].amount.is_some());
                assert_eq!(postings[1].account.join(), "Income:Gross");
                assert!(postings[1].amount.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_flow_with_alias() {
        let prog = parse_prog(
            "entry monthly \"Jim's paycheck\" { Assets:Cash = 1000\nIncome:Gross } as paycheck",
        );
        match &prog.decls[0].0 {
            Decl::Entry { label, alias, .. } => {
                assert_eq!(label, "Jim's paycheck");
                assert_eq!(alias.as_deref(), Some("paycheck"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_posting_all() {
        let prog = parse_prog(
            "entry monthly \"loan payment\" { Liabilities:AccruedInterest = all\nAssets:Cash }",
        );
        match &prog.decls[0].0 {
            Decl::Entry { postings, .. } => {
                assert!(matches!(&postings[0].amount, Some(PostingAmount::All)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_assert() {
        let prog = parse_prog("assert that Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert { schedule: None, .. }
        ));
    }

    #[test]
    fn parses_scheduled_assert() {
        let prog = parse_prog("assert yearly that Assets:Retirement >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert {
                schedule: Some(ScheduleRef::Literal(_)),
                ..
            }
        ));

        let prog = parse_prog("assert quarterly that Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert {
                schedule: Some(ScheduleRef::Literal(_)),
                ..
            }
        ));

        let prog = parse_prog("assert monthly that Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert {
                schedule: Some(ScheduleRef::Literal(_)),
                ..
            }
        ));
    }

    #[test]
    fn parses_min_call() {
        let prog = parse_prog("entry monthly \"f\" { A:B = min(A:Cash, 2_000)\nC:D }");
        match &prog.decls[0].0 {
            Decl::Entry { postings, .. } => {
                let Some(PostingAmount::Expr((e, _))) = &postings[0].amount else {
                    panic!("expected expr amount");
                };
                assert!(
                    matches!(e.as_ref(), Expr::Call(name, args) if name == "min" && args.len() == 2)
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_if_expr() {
        let prog = parse_prog("assert that if Assets:Cash > 0 then 1 else 0");
        if let Decl::Assert {
            asserted: (e, _), ..
        } = &prog.decls[0].0
        {
            assert!(matches!(e.as_ref(), Expr::If { .. }));
        } else {
            panic!("expected if expression");
        }
    }

    fn parse_errs(src: &str) -> Vec<Diagnostic> {
        let Ok(tokens) = Lexer::new(src).lex() else {
            panic!("lexer errored")
        };
        Parser::new(tokens).parse().unwrap_err()
    }

    #[test]
    fn if_requires_then() {
        let errs = parse_errs("assert that if Assets:Cash > 0 1 else 0");
        assert!(errs.iter().any(|d| d.message.contains("then")));
    }

    #[test]
    fn if_requires_else() {
        let errs = parse_errs("assert that if Assets:Cash > 0 then 1 0");
        assert!(errs.iter().any(|d| d.message.contains("else")));
    }
}
