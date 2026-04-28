use crate::{lexer::Token, Span};
use chumsky::{input::ValueInput, prelude::*};

pub fn ident_ci<'src, I>(
    s: &'static str,
) -> impl Parser<'src, I, (), extra::Err<Rich<'src, Token<'src>, Span>>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = Span>,
{
    select! { Token::Ident(id) if id.eq_ignore_ascii_case(s) => () }
}

pub fn comma_list<'src, I, O, P>(
    item: P,
) -> impl Parser<'src, I, Vec<O>, extra::Err<Rich<'src, Token<'src>, Span>>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = Span>,
    O: Clone,
    P: Parser<'src, I, O, extra::Err<Rich<'src, Token<'src>, Span>>> + Clone,
{
    item.clone()
        .then(
            just(Token::Comma)
                .ignore_then(item.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(
            just(Token::Comma)
                .or_not()
                .ignore_then(ident_ci("and").ignore_then(item).or_not()),
        )
        .map(|((first, middle), last)| {
            let mut v = vec![first];
            v.extend(middle);
            if let Some(x) = last {
                v.push(x);
            }
            v
        })
}
