//! Lexer: surface bytes → tokens with byte spans. [FRONT-2, D8, D3]
//!
//! Encodes the Prolog lexical convention as distinct token kinds (`Var` for
//! Uppercase/`_`-leading, `Const` for lowercase). Whitespace is skipped; `%`
//! comments are tokenized (`Comment`) so the parser can attach them as trivia
//! (FRONT-10). Lexing never fails hard: an unknown byte becomes a diagnostic and
//! the scan continues, so the parser still sees a stream.

use logos::Logos;
use strata_ir::diag::Span;

use crate::diagnostics::{codes, Diagnostics};

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
pub enum Tok {
    // reserved words
    #[token("pred")]
    Pred,
    #[token("domain")]
    Domain,
    #[token("input")]
    Input,
    #[token("from")]
    From,
    #[token("model")]
    Model,
    #[token("neural")]
    Neural,
    #[token("not")]
    Not,
    #[token("int")]
    Int,
    #[token("term")]
    Term,
    #[token("min")]
    Min,
    #[token("max")]
    Max,
    #[token("sum")]
    Sum,
    #[token("count")]
    Count,
    #[token("prob_or")]
    ProbOr,
    #[token("Bool")]
    Bool,
    #[token("Trop")]
    Trop,
    #[token("Prov")]
    Prov,
    #[token("Prov_k")]
    ProvK,
    #[token("total")]
    Total,
    #[token("partial")]
    Partial,
    #[token("complete")]
    Complete,
    #[token("sound_only")]
    SoundOnly,
    #[token("deterministic")]
    Deterministic,
    #[token("stochastic")]
    Stochastic,

    // query / pragma prefixes (longer forms first)
    #[token("?prob")]
    QProb,
    #[token("?grad")]
    QGrad,
    #[token("?")]
    Question,
    #[token("@terms")]
    AtTerms,
    #[token("@asp")]
    AtAsp,

    // punctuation
    #[token(":-")]
    ColonDash,
    #[token("::")]
    DoubleColon,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
    #[token(",")]
    Comma,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,

    // literals / identifiers
    #[regex(r"[A-Z_][A-Za-z0-9_]*", |l| l.slice().to_string())]
    Var(String),
    #[regex(r"[a-z][A-Za-z0-9_]*", |l| l.slice().to_string())]
    Const(String),
    #[regex(r"-?[0-9]+\.[0-9]+", |l| l.slice().parse::<f64>().ok())]
    Float(f64),
    #[regex(r"-?[0-9]+", |l| l.slice().parse::<i64>().ok())]
    IntLit(i64),
    #[regex(r#""[^"]*""#, |l| { let s = l.slice(); s[1..s.len()-1].to_string() })]
    Str(String),

    /// A `%`-to-end-of-line comment, retained verbatim as trivia (FRONT-10, D12).
    #[regex(r"%[^\n]*", |l| l.slice().trim_end().to_string())]
    Comment(String),
}

impl Tok {
    /// Human name for diagnostics (`expected …, found <name>`).
    pub fn describe(&self) -> String {
        use Tok::*;
        match self {
            Var(s) => format!("variable `{s}`"),
            Const(s) => format!("identifier `{s}`"),
            IntLit(n) => format!("integer `{n}`"),
            Float(n) => format!("number `{n}`"),
            Str(s) => format!("string {s:?}"),
            Comment(_) => "comment".into(),
            ColonDash => "`:-`".into(),
            Dot => "`.`".into(),
            other => format!("`{other:?}`"),
        }
    }
}

/// A token with its byte span.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub tok: Tok,
    pub span: Span,
}

/// Tokenize `src`, recording a diagnostic for each unrecognized byte run.
pub fn lex(src: &str, diags: &mut Diagnostics) -> Vec<Spanned> {
    let mut out = Vec::new();
    let mut lex = Tok::lexer(src);
    while let Some(res) = lex.next() {
        let span = lex.span();
        let span = Span {
            start: span.start as u32,
            end: span.end as u32,
        };
        match res {
            Ok(tok) => out.push(Spanned { tok, span }),
            Err(()) => diags.error(
                codes::LEX_UNEXPECTED,
                format!("unexpected character(s) {:?}", lex.slice()),
                span,
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        let mut d = Diagnostics::new();
        let toks = lex(src, &mut d)
            .into_iter()
            .map(|s| s.tok)
            .collect::<Vec<_>>();
        assert!(!d.has_errors(), "unexpected lex errors: {:?}", d);
        toks
    }

    #[test]
    fn var_const_split() {
        assert_eq!(
            kinds("X y _Z ab1"),
            vec![
                Tok::Var("X".into()),
                Tok::Const("y".into()),
                Tok::Var("_Z".into()),
                Tok::Const("ab1".into()),
            ]
        );
    }

    #[test]
    fn keywords_beat_idents_but_prefixes_do_not() {
        assert_eq!(kinds("Bool"), vec![Tok::Bool]);
        // longer identifier with a keyword prefix stays an identifier
        assert_eq!(kinds("Boolean"), vec![Tok::Var("Boolean".into())]);
        assert_eq!(kinds("mint"), vec![Tok::Const("mint".into())]);
    }

    #[test]
    fn punctuation_and_numbers() {
        assert_eq!(
            kinds("X :- p(a, 5). -3 0.5 ::"),
            vec![
                Tok::Var("X".into()),
                Tok::ColonDash,
                Tok::Const("p".into()),
                Tok::LParen,
                Tok::Const("a".into()),
                Tok::Comma,
                Tok::IntLit(5),
                Tok::RParen,
                Tok::Dot,
                Tok::IntLit(-3),
                Tok::Float(0.5),
                Tok::DoubleColon,
            ]
        );
    }

    #[test]
    fn comments_are_tokenized_as_trivia() {
        assert_eq!(
            kinds("a % comment\n  b"),
            vec![
                Tok::Const("a".into()),
                Tok::Comment("% comment".into()),
                Tok::Const("b".into()),
            ]
        );
    }
}
