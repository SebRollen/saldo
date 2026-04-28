mod schedule;
mod util;

use crate::ast::{
    AggKind, BinOp, Decl, Expr, Interval, ParamBody, Path, Posting, PostingAmount, Program,
    ScheduleKind, SpannedExpr,
};
use crate::{lexer::Token, Span};
use chumsky::{input::ValueInput, prelude::*};
use schedule::parse_schedule;

pub fn parser<'src, I>(
) -> impl Parser<'src, I, Program, extra::Err<Rich<'src, Token<'src>, Span>>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = Span>,
{
    // ---------- atoms & helpers ----------

    let ident = select! { Token::Ident(s) => s };

    let boolean = select! {
        Token::False => false,
        Token::True => true
    };

    let number = select! {
        Token::Float(f) => f,
    };

    let date = select! { Token::Date(d) => d }.labelled("date");

    // Colon-separated path: `Assets:Cash`, `Income:Gross`, `Assets:401k`, etc.
    let colon_path = ident
        .map(|i| i.to_string())
        .then(
            just(Token::Colon)
                .ignore_then(ident)
                .map(|i| i.to_string())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest): (String, Vec<String>)| {
            let mut v = Vec::with_capacity(1 + rest.len());
            v.push(first);
            v.extend(rest);
            Path(v)
        });

    // Unit annotation: `usd`, `usd/year`, `%`.
    let unit_atom = choice((
        ident.map(|s: &str| s.to_string()),
        just(Token::Percent).to("%".to_string()),
    ));
    let unit = unit_atom
        .clone()
        .then(just(Token::Slash).ignore_then(unit_atom).or_not())
        .map(|(a, b): (String, Option<String>)| match b {
            Some(b) => format!("{a}/{b}"),
            None => a,
        });

    // ---------- expression grammar ----------

    let expr = recursive(|expr| {
        let args = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>();

        let call = ident
            .then(args.delimited_by(just(Token::LParen), just(Token::RParen)))
            .map_with(|(name, args): (&str, Vec<SpannedExpr>), e| {
                (Box::new(Expr::Call(name.to_string(), args)), e.span())
            });

        let if_expr = just(Token::Ident("if"))
            .ignore_then(expr.clone())
            .then_ignore(just(Token::Ident("then")))
            .then(expr.clone())
            .then_ignore(just(Token::Ident("else")))
            .then(expr.clone())
            .map_with(
                |((cond, then), else_): ((SpannedExpr, SpannedExpr), SpannedExpr), e| {
                    (Box::new(Expr::If { cond, then, else_ }), e.span())
                },
            );

        let agg_kind = select! {
            Token::Ident("ytd") => AggKind::Ytd,
            Token::Ident("qtd") => AggKind::Qtd,
            Token::Ident("mtd") => AggKind::Mtd,
        };

        // After the first `ident .`, the rest is either:
        //   `ident . agg_kind`  → qualified:   flow=first, leg=second
        //   `agg_kind`          → unqualified: leg=first
        let calc_rest = choice((
            ident
                .then_ignore(just(Token::Period))
                .then(agg_kind)
                .map(|(leg, kind): (&str, AggKind)| (Some(leg.to_string()), kind)),
            agg_kind.map(|kind| (None, kind)),
        ));
        let calc = ident
            .then_ignore(just(Token::Period))
            .then(calc_rest)
            .map_with(|(first, (leg_opt, kind)), e| {
                let (flow, leg) = match leg_opt {
                    Some(leg) => (Some(first.to_string()), leg),
                    None => (None, first.to_string()),
                };
                (Box::new(Expr::ParamAgg(flow, leg, kind)), e.span())
            });

        let atom = choice((
            if_expr,
            boolean.map_with(|b, e| (Box::new(Expr::Bool(b)), e.span())),
            number.map_with(|v, e| (Box::new(Expr::Num(v)), e.span())),
            call,
            calc,
            colon_path
                .clone()
                .map_with(|p, e| (Box::new(Expr::Ref(p)), e.span())),
            expr.clone()
                .delimited_by(just(Token::LParen), just(Token::RParen)),
        ));

        let unary = just(Token::Minus)
            .map_with(|_, e| e.span())
            .repeated()
            .foldr(atom, |minus_span: Span, inner: SpannedExpr| {
                let span = SimpleSpan::from(minus_span.start..inner.1.end);
                (Box::new(Expr::Neg(inner)), span)
            });

        let product_op = choice((
            just(Token::Star).to(BinOp::Mul),
            just(Token::Slash).to(BinOp::Div),
        ));
        let product = unary.clone().foldl(
            product_op.then(unary).repeated(),
            |l: SpannedExpr, (op, r): (BinOp, SpannedExpr)| {
                let span = SimpleSpan::from(l.1.start..r.1.end);
                (Box::new(Expr::Bin(l, op, r)), span)
            },
        );

        let sum_op = choice((
            just(Token::Plus).to(BinOp::Add),
            just(Token::Minus).to(BinOp::Sub),
        ));
        let sum = product.clone().foldl(
            sum_op.then(product).repeated(),
            |l: SpannedExpr, (op, r): (BinOp, SpannedExpr)| {
                let span = SimpleSpan::from(l.1.start..r.1.end);
                (Box::new(Expr::Bin(l, op, r)), span)
            },
        );

        let cmp_op = choice((
            just(Token::LtEq).to(BinOp::Le),
            just(Token::GtEq).to(BinOp::Ge),
            just(Token::EqEq).to(BinOp::Eq),
            just(Token::LessThan).to(BinOp::Lt),
            just(Token::MoreThan).to(BinOp::Gt),
        ));
        sum.clone().foldl(
            cmp_op.then(sum).repeated(),
            |l: SpannedExpr, (op, r): (BinOp, SpannedExpr)| {
                let span = SimpleSpan::from(l.1.start..r.1.end);
                (Box::new(Expr::Bin(l, op, r)), span)
            },
        )
    });

    // ---------- declarations ----------

    // `account <colon_path> [= <expr>]`
    let account_decl = just(Token::Ident("account"))
        .ignore_then(colon_path.clone())
        .then(just(Token::Eq).ignore_then(expr.clone()).or_not())
        .map(|(name, init)| Decl::Account { name, init });

    // `schedule <schedule>`
    let schedule_decl = just(Token::Ident("schedule"))
        .ignore_then(ident)
        .then_ignore(just(Token::Eq))
        .then(parse_schedule())
        .map(|(name, schedule)| Decl::Schedule {
            name: name.to_string(),
            schedule,
        });

    // `from <date> [to <date>] = <expr>`
    let interval = just(Token::Ident("from"))
        .ignore_then(date)
        .then(just(Token::Ident("to")).ignore_then(date).or_not())
        .then_ignore(just(Token::Eq))
        .then(expr.clone())
        .map(|((from, to), value)| Interval { from, to, value });

    let const_body = just(Token::Eq)
        .ignore_then(expr.clone())
        .map(ParamBody::Const);

    let schedule_body = interval
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .map(ParamBody::Schedule);

    // `param <ident> [: <unit>] (= <expr> | { <interval>* })`
    let param_decl = just(Token::Ident("param"))
        .ignore_then(ident)
        .then(just(Token::Colon).ignore_then(unit.clone()).or_not())
        .then(choice((const_body, schedule_body)))
        .map(
            |((name, unit), body): ((&str, Option<String>), ParamBody)| Decl::Param {
                name: name.to_string(),
                unit,
                body,
            },
        );

    let sched_kind = choice((
        just(Token::Ident("daily")).to(ScheduleKind::Daily),
        just(Token::Ident("monthly")).to(ScheduleKind::Monthly),
        just(Token::Ident("quarterly")).to(ScheduleKind::Quarterly),
        just(Token::Ident("yearly")).to(ScheduleKind::Yearly),
        just(Token::Ident("on"))
            .ignore_then(date)
            .map(ScheduleKind::On),
    ));

    // A posting amount: `all` or an expression.
    let posting_amount = choice((
        just(Token::Ident("all")).to(PostingAmount::All),
        expr.clone().map(PostingAmount::Expr),
    ));

    // A posting line: `<colon_path> [<posting_amount>] [as <ident>]`
    // No amount → auto-balance leg (amount = None).
    // Postings within a flow body are comma-separated.
    let posting = colon_path
        .clone()
        .then(just(Token::Eq).ignore_then(posting_amount).or_not())
        .then(just(Token::Ident("as")).ignore_then(ident).or_not())
        .map(
            |((account, amount), leg_name): ((Path, Option<PostingAmount>), Option<&str>)| {
                Posting {
                    account,
                    amount,
                    leg_name: leg_name.map(|s| s.to_string()),
                }
            },
        );

    // `assert [<sched_kind>] <expr>` — schedule defaults to Daily.
    let assert_decl = just(Token::Ident("assert"))
        .ignore_then(sched_kind.clone().or_not())
        .then(expr.clone())
        .map(|(sched, e)| Decl::Assert(sched.unwrap_or(ScheduleKind::Daily), e));

    let str_lit = select! { Token::Str(s) => s };

    // `<sched_kind> <str_lit> { <posting>* } [as <ident>]`
    let flow_decl = sched_kind
        .then(str_lit)
        .then(
            posting
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .then(just(Token::Ident("as")).ignore_then(ident).or_not())
        .map(|(((schedule, label), postings), alias)| Decl::Flow {
            label,
            alias: alias.map(|s: &str| s.to_string()),
            schedule,
            postings,
        });

    let decl = choice((
        account_decl,
        schedule_decl,
        param_decl,
        assert_decl,
        flow_decl,
    ))
    .map_with(|d, e| (d, e.span()));

    decl.repeated()
        .collect::<Vec<_>>()
        .then_ignore(end())
        .map(|decls| Program { decls })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lexer;
    use chrono::NaiveDate;
    use chumsky::Parser;

    fn parse(src: &str) -> Program {
        let (tokens, lex_errs) = lexer().parse(src).into_output_errors();
        assert!(lex_errs.is_empty(), "lex errs: {lex_errs:?}");
        let tokens = tokens.unwrap();
        let eoi = (src.len()..src.len()).into();
        let input = tokens.as_slice().map(eoi, |(t, s)| (t, s));
        let (prog, errs) = parser().parse(input).into_output_errors();
        assert!(errs.is_empty(), "parse errs: {errs:?}");
        prog.unwrap()
    }

    #[test]
    fn parses_account_with_init() {
        let prog = parse("account Liabilities:Loan = -3_000_000");
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
        let prog = parse("account Assets:Cash");
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
        let prog = parse("param interest_rate = 0.05");
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
        let prog = parse(
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
        let prog = parse("monthly \"paycheck\" { Assets:Cash = salary_rate / 12\nIncome:Gross }");
        match &prog.decls[0].0 {
            Decl::Flow {
                label,
                alias,
                schedule,
                postings,
            } => {
                assert_eq!(label, "paycheck");
                assert!(alias.is_none());
                assert!(matches!(schedule, ScheduleKind::Monthly));
                assert_eq!(postings.len(), 2);
                assert_eq!(postings[0].account.join(), "Assets:Cash");
                assert!(postings[0].amount.is_some());
                assert_eq!(postings[1].account.join(), "Income:Gross");
                assert!(postings[1].amount.is_none()); // auto-balance
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_flow_with_alias() {
        let prog =
            parse("monthly \"Jim's paycheck\" { Assets:Cash = 1000\nIncome:Gross } as paycheck");
        match &prog.decls[0].0 {
            Decl::Flow { label, alias, .. } => {
                assert_eq!(label, "Jim's paycheck");
                assert_eq!(alias.as_deref(), Some("paycheck"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_posting_all() {
        let prog =
            parse("monthly \"loan payment\" { Liabilities:AccruedInterest = all\nAssets:Cash }");
        match &prog.decls[0].0 {
            Decl::Flow { postings, .. } => {
                assert!(matches!(&postings[0].amount, Some(PostingAmount::All)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_assert() {
        let prog = parse("assert Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert(ScheduleKind::Daily, _)
        ));
    }

    #[test]
    fn parses_scheduled_assert() {
        let prog = parse("assert yearly Assets:Retirement >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert(ScheduleKind::Yearly, _)
        ));

        let prog = parse("assert quarterly Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert(ScheduleKind::Quarterly, _)
        ));

        let prog = parse("assert monthly Assets:Cash >= 0");
        assert!(matches!(
            prog.decls[0].0,
            Decl::Assert(ScheduleKind::Monthly, _)
        ));
    }

    #[test]
    fn parses_min_call() {
        let prog = parse("monthly \"f\" { A:B = min(A:Cash, 2_000)\nC:D }");
        match &prog.decls[0].0 {
            Decl::Flow { postings, .. } => {
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
        let prog = parse("assert if Assets:Cash > 0 then 1 else 0");
        if let Decl::Assert(_, (e, _)) = &prog.decls[0].0 {
            assert!(matches!(e.as_ref(), Expr::If { .. }));
        } else {
            panic!("expected if expression");
        }
    }
}
