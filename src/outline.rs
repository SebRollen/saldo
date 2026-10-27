//! What a model declares and where it's written, for tools that show a model
//! or change a number in it without reading the rest.

use crate::ast::{
    Arm, Change, Decl, EntryStmt, Expr, LetBody, ModBody, Program, ScenarioBody, Span, SpannedExpr,
    Stmt,
};
use chrono::NaiveDate;

/// What a model declares, in the order it's written, followed by the changes
/// its scenarios make. For a model read with [`crate::load`], spans are byte
/// ranges of its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub items: Vec<Item>,
    /// Every number and bool written in the model, in the order written,
    /// with its `%` and its minus sign, if it has one of its own. Text that
    /// replaces one changes it and nothing else.
    pub literals: Vec<Span>,
}

/// A declaration, or a change a scenario makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// Its name from the top of the model, like `loans.rate`. A check or a
    /// `balances` block has no name of its own, so this is its module's,
    /// which is empty at the top.
    pub name: String,
    /// The scenario that makes it, if it's a change.
    pub scenario: Option<String>,
    /// All of it, from its keyword.
    pub span: Span,
    /// The expressions that say what it's worth: a value's expression or its
    /// arms, the lines of a `balances` block, or the arguments of the call
    /// that makes a module. Empty for the other kinds.
    pub parts: Vec<Part>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `let`.
    Value,
    /// `fn`.
    Function,
    /// `entry`.
    Entry,
    /// `check`.
    Check,
    /// `balances`, on this date.
    Balances(NaiveDate),
    /// `mod x = f(…)`. A module written out isn't an item: what it declares
    /// is, under its name.
    Module,
    /// `drop`, in a scenario. Its name is what it drops.
    Drop,
}

/// One of the expressions an item is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    /// The part with what it's for: an arm from its days to its growth, a
    /// line of balances from its account, an argument from its name.
    pub span: Span,
    /// The expression.
    pub value: Span,
    /// The number or bool in the expression, if it's nothing more than one
    /// with a unit: the `120_000` of `120_000/year`, or the `-5%` of `-5%`.
    /// Text that replaces it changes the part and nothing else.
    pub literal: Option<Span>,
}

impl Part {
    fn of(span: Span, value: &SpannedExpr) -> Self {
        Part {
            span,
            value: value.1,
            literal: literal(value),
        }
    }
}

fn literal(expr: &SpannedExpr) -> Option<Span> {
    match expr.0.as_ref() {
        Expr::Num(_) | Expr::Bool(_) => Some(expr.1),
        Expr::Neg(inner) if matches!(inner.0.as_ref(), Expr::Num(_)) => Some(expr.1),
        Expr::Per(inner, _) | Expr::Period(inner, _) => literal(inner),
        _ => None,
    }
}

pub(crate) fn outline(program: &Program) -> Outline {
    let mut items = Vec::new();
    let mut literals = Vec::new();
    for (decl, span) in &program.decls {
        declare(&mut items, None, &mut Vec::new(), decl, *span);
        decl_literals(decl, &mut literals);
    }
    for scenario in &program.scenarios {
        let ScenarioBody::Inline(changes) = &scenario.body else {
            continue;
        };
        let name = Some(scenario.name.0.as_str());
        for (change, span) in changes {
            let in_module = |module: &[(String, Span)]| -> Vec<String> {
                module.iter().map(|(part, _)| part.clone()).collect()
            };
            match change {
                Change::Decl { module, decl } => {
                    declare(&mut items, name, &mut in_module(module), decl, *span);
                    decl_literals(decl, &mut literals);
                }
                Change::Drop { module, path, when } => {
                    if let Some(when) = when {
                        expr_literals(when, &mut literals);
                    }
                    let mut dropped = in_module(module);
                    dropped.extend(path.0.0.iter().cloned());
                    items.push(Item {
                        kind: Kind::Drop,
                        name: dropped.join("."),
                        scenario: name.map(str::to_string),
                        span: *span,
                        parts: Vec::new(),
                    });
                }
            }
        }
    }
    literals.sort_by_key(|span| span.start);
    Outline { items, literals }
}

/// Adds the numbers and bools written in `decl` to `out`.
fn decl_literals(decl: &Decl, out: &mut Vec<Span>) {
    match decl {
        Decl::Let { body, .. } => match body {
            LetBody::Expr(value) => expr_literals(value, out),
            LetBody::Arms(arms) => {
                for arm in arms {
                    let growth = arm.growth.as_ref().map(|growth| &growth.by);
                    for e in arm.when.iter().chain([&arm.value]).chain(growth) {
                        expr_literals(e, out);
                    }
                }
            }
        },
        Decl::Fn { body, .. } => {
            for stmt in body {
                match stmt {
                    Stmt::Let { value, .. } | Stmt::Return(value) => expr_literals(value, out),
                    Stmt::Mod(decls) => {
                        for (decl, _) in decls {
                            decl_literals(decl, out);
                        }
                    }
                }
            }
        }
        Decl::Entry { when, body, .. } => {
            if let Some(when) = when {
                expr_literals(when, out);
            }
            EntryStmt::visit(body, &mut |stmt| match stmt {
                EntryStmt::Let { value, .. } => expr_literals(value, out),
                EntryStmt::Move(m) => expr_literals(&m.amount, out),
                EntryStmt::If { branches, .. } => {
                    for (cond, _) in branches {
                        expr_literals(cond, out);
                    }
                }
                EntryStmt::For { .. } => {}
            });
        }
        Decl::Check { expr, when } => {
            for e in std::iter::once(expr).chain(when) {
                expr_literals(e, out);
            }
        }
        Decl::Balances { balances, .. } => {
            for line in balances {
                expr_literals(&line.value, out);
            }
        }
        Decl::Mod { body, .. } => match body {
            ModBody::Inline(decls) => {
                for (decl, _) in decls {
                    decl_literals(decl, out);
                }
            }
            ModBody::Call(call) => expr_literals(call, out),
            ModBody::File => {}
        },
    }
}

/// Adds the numbers and bools written in `expr` to `out`: a minus sign
/// right before a number is the number's.
fn expr_literals(expr: &SpannedExpr, out: &mut Vec<Span>) {
    match expr.0.as_ref() {
        Expr::Num(_) | Expr::Bool(_) => out.push(expr.1),
        Expr::Neg(x) if matches!(x.0.as_ref(), Expr::Num(_)) => out.push(expr.1),
        _ => crate::resolver::walk_children(expr, &mut |child| expr_literals(child, out)),
    }
}

/// Adds `decl`, written in `module`, or what it declares if it's a module
/// written out.
fn declare(
    items: &mut Vec<Item>,
    scenario: Option<&str>,
    module: &mut Vec<String>,
    decl: &Decl,
    span: Span,
) {
    let named = |module: &[String], name: &str| -> String {
        let parts = module.iter().map(String::as_str).chain([name]);
        parts.collect::<Vec<_>>().join(".")
    };
    let (kind, name, parts) = match decl {
        Decl::Let { name, body } => {
            let parts = match body {
                LetBody::Expr(value) => vec![Part::of(value.1, value)],
                LetBody::Arms(arms) => arms
                    .iter()
                    .map(|Arm { value, span, .. }| Part::of(*span, value))
                    .collect(),
            };
            (Kind::Value, named(module, &name.0), parts)
        }
        Decl::Fn { name, .. } => (Kind::Function, named(module, &name.0), Vec::new()),
        Decl::Entry { name, .. } => (Kind::Entry, named(module, &name.0), Vec::new()),
        Decl::Check { .. } => (Kind::Check, module.join("."), Vec::new()),
        Decl::Balances { date, balances } => {
            let parts = balances.iter().map(|line| {
                let span = Span::new(line.target.1.start, line.value.1.end);
                Part::of(span, &line.value)
            });
            (Kind::Balances(*date), module.join("."), parts.collect())
        }
        Decl::Mod { name, body } => match body {
            ModBody::Inline(decls) => {
                module.push(name.0.clone());
                for (decl, span) in decls {
                    declare(items, scenario, module, decl, *span);
                }
                module.pop();
                return;
            }
            ModBody::Call(call) => {
                let Expr::Call(_, args) = call.0.as_ref() else {
                    unreachable!("the parser only takes a call here");
                };
                let parts = args.iter().map(|arg| match arg.0.as_ref() {
                    Expr::Named(_, value) => Part::of(arg.1, value),
                    _ => Part::of(arg.1, arg),
                });
                (Kind::Module, named(module, &name.0), parts.collect())
            }
            // Loading replaces these with what's in their files.
            ModBody::File => return,
        },
    };
    items.push(Item {
        kind,
        name,
        scenario: scenario.map(str::to_string),
        span,
        parts,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "\
balances @ 2026-01-01 {
  assets.cash = 5_000
  pay.paycheck.pension[year] = 2 * 1_000
}

let salary = 95_000/year   // before tax
let raise = -1.5%
let prices {
  2026.. => 1 compounding 3% every year
  _      => (1 + 0)
}

fn half(x) { x / 2 }

fn loan(amount, rate) {
  mod {
    let owed = amount
  }
}

mod car = loan(20_000, rate: 6%/year)

mod pay {
  let moved = false
  let bonus = salary * 10%
  entry paycheck \"Paycheck\" @ last {
    income.salary -> assets.cash : salary #pension
  }
  check assets.cash >= 0

  scenario lean {
    let bonus = 0/year
    drop paycheck @ 2027..
  }
}

scenario lean {
  let pay.moved { 2027.. => true  _ => false }
  check assets.cash >= 1_000
}
";

    fn outline() -> Outline {
        crate::load(MODEL).unwrap().outline()
    }

    fn text(span: Span) -> &'static str {
        &MODEL[span.into_range()]
    }

    /// Each item as its kind, its scenario and its name, then its parts'
    /// expressions with the literal in each in brackets.
    fn described(outline: &Outline) -> Vec<String> {
        let items = outline.items.iter().map(|item| {
            let parts = item.parts.iter().map(|part| match part.literal {
                Some(literal) => format!(
                    "{}[{}]{}",
                    &MODEL[part.value.start..literal.start],
                    text(literal),
                    &MODEL[literal.end..part.value.end]
                ),
                None => text(part.value).to_string(),
            });
            let scenario = item.scenario.as_deref().unwrap_or("-");
            let parts = parts.collect::<Vec<_>>().join(" | ");
            format!("{:?} {scenario} `{}`: {parts}", item.kind, item.name)
        });
        items.collect()
    }

    #[test]
    fn lists_what_a_model_declares() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            described(&outline()),
            [
                format!("Balances({date}) - ``: [5_000] | 2 * 1_000"),
                "Value - `salary`: [95_000]/year".to_string(),
                "Value - `raise`: [-1.5%]".to_string(),
                "Value - `prices`: [1] | (1 + 0)".to_string(),
                "Function - `half`: ".to_string(),
                "Function - `loan`: ".to_string(),
                "Module - `car`: [20_000] | [6%]/year".to_string(),
                "Value - `pay.moved`: [false]".to_string(),
                "Value - `pay.bonus`: salary * 10%".to_string(),
                "Entry - `pay.paycheck`: ".to_string(),
                "Check - `pay`: ".to_string(),
                "Value lean `pay.bonus`: [0]/year".to_string(),
                "Drop lean `pay.paycheck`: ".to_string(),
                "Value lean `pay.moved`: [true] | [false]".to_string(),
                "Check lean ``: ".to_string(),
            ]
        );
    }

    #[test]
    fn spans_cover_what_is_written() {
        let outline = outline();
        let item = |name: &str| {
            let mut items = outline.items.iter();
            items.find(|item| item.name == name).unwrap()
        };
        assert_eq!(text(item("salary").span), "let salary = 95_000/year");
        let prices = item("prices");
        assert!(text(prices.span).starts_with("let prices {"));
        assert!(text(prices.span).ends_with('}'));
        assert_eq!(
            text(prices.parts[0].span),
            "2026.. => 1 compounding 3% every year"
        );
        assert_eq!(text(prices.parts[1].span), "_      => (1 + 0)");
        let balances = &outline.items[0];
        assert_eq!(text(balances.parts[0].span), "assets.cash = 5_000");
        assert_eq!(
            text(balances.parts[1].span),
            "pay.paycheck.pension[year] = 2 * 1_000"
        );
        assert_eq!(text(item("car").parts[1].span), "rate: 6%/year");
        let dropped = outline.items.iter().find(|item| item.kind == Kind::Drop);
        assert_eq!(text(dropped.unwrap().span), "drop paycheck @ 2027..");
    }

    #[test]
    fn lists_every_number_written() {
        let literals: Vec<&str> = outline().literals.into_iter().map(text).collect();
        assert_eq!(
            literals,
            [
                "5_000", "2", "1_000", "95_000", "-1.5%", "2026", "1", "3%", "1", "0", "2",
                "20_000", "6%", "false", "10%", "0", "0", "2027", "2027", "true", "false", "1_000",
            ]
        );
    }

    #[test]
    fn a_literal_can_be_replaced_where_it_is() {
        let outline = outline();
        let literals = outline.items.iter().flat_map(|item| &item.parts);
        let mut literals: Vec<Span> = literals.filter_map(|part| part.literal).collect();
        // From the end, so that the spans before a change still hold.
        literals.sort_by_key(|span| std::cmp::Reverse(span.start));
        let mut changed = MODEL.to_string();
        for span in &literals {
            let new = match text(*span) {
                "true" => "false",
                "false" => "true",
                _ => "7",
            };
            changed.replace_range(span.into_range(), new);
        }
        let after = crate::load(&changed).unwrap().outline();
        assert_eq!(after.items.len(), outline.items.len());
        for (before, after) in outline.items.iter().zip(&after.items) {
            assert_eq!((before.kind, &before.name), (after.kind, &after.name));
            assert_eq!(before.parts.len(), after.parts.len());
            for (before, after) in before.parts.iter().zip(&after.parts) {
                assert_eq!(before.literal.is_some(), after.literal.is_some());
            }
        }
    }
}
