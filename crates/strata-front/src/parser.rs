//! Recursive-descent parser: surface → High-IR. [FRONT-4/5/6/7, D2, D3, D5]
//!
//! Follows docs/grammar.ebnf. Produces the High-IR `Program` owned by strata-ir.
//! Recovery: on a clause error, skip to the next `.` so one bad clause does not
//! blank the file. Whole-language constructs outside Phase-0 execution parse into
//! valid IR and then get a stable "not implemented in Phase 0" diagnostic (D5),
//! never a syntax error. Singleton variables are flagged with a `_`-rename fix
//! (D3); undeclared-predicate detection is strata-check's job (CHECK-2).

use strata_ir::diag::{FixPatch, Span};
use strata_ir::high::program::{
    AggOp, Atom, DomainDecl, Fact, InputDecl, Item, ItemKind, Literal, NeuralSpec, Pragma,
    PredDecl, Program, Query, QueryKind, Term,
};
use strata_ir::high::sig::{
    Annotation, ArgType, Completeness, Determinism, Effects, Signature, Termination,
};
use strata_ir::trop::Weight;

use crate::diagnostics::{codes, Diagnostics};
use crate::lexer::{lex, Spanned, Tok};

/// Parse surface source into a High-IR program plus its diagnostics.
pub fn parse(src: &str) -> (Program, Diagnostics) {
    let mut diags = Diagnostics::new();
    let toks = lex(src, &mut diags);
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        diags: &mut diags,
        src,
        src_len: src.len() as u32,
        vars: Vec::new(),
    };
    let mut items = Vec::new();
    let mut prev_end: u32 = 0;
    while p.cur().is_some() {
        // A blank line separating this block from the previous item (FRONT-10/D12):
        // ≥2 newlines in the gap before the item (or its leading comments).
        let block_start = p.toks.get(p.pos).map(|s| s.span.start).unwrap_or(p.src_len);
        let blank_before = p.blank_gap(prev_end, block_start);
        // Comments preceding an item become its leading trivia (FRONT-10, D12).
        let leading = p.take_leading_comments();
        if p.cur().is_none() {
            break; // only trailing comments remained (dropped at EOF)
        }
        let start = p.cur_span().start;
        match p.item() {
            Ok(mut item) => {
                item.trivia.leading = leading;
                item.trivia.blank_before = blank_before;
                item.span = Span {
                    start,
                    end: p.prev_end(),
                };
                prev_end = item.span.end;
                items.push(item);
            }
            Err(()) => p.recover(),
        }
    }
    (Program::new(items), diags)
}

struct Parser<'a> {
    toks: &'a [Spanned],
    pos: usize,
    diags: &'a mut Diagnostics,
    src: &'a str,
    src_len: u32,
    /// Variable occurrences of the rule currently being parsed (for singletons).
    vars: Vec<(String, Span)>,
}

type PResult<T> = Result<T, ()>;

impl<'a> Parser<'a> {
    /// Index of the next non-comment token at/after `pos` (comments are trivia).
    fn cur_idx(&self) -> Option<usize> {
        let mut i = self.pos;
        while matches!(
            self.toks.get(i),
            Some(Spanned {
                tok: Tok::Comment(_),
                ..
            })
        ) {
            i += 1;
        }
        (i < self.toks.len()).then_some(i)
    }
    fn cur(&self) -> Option<&Tok> {
        self.cur_idx().map(|i| &self.toks[i].tok)
    }
    fn cur_spanned(&self) -> Option<&Spanned> {
        self.cur_idx().map(|i| &self.toks[i])
    }
    fn cur_span(&self) -> Span {
        self.cur_idx().map(|i| self.toks[i].span).unwrap_or(Span {
            start: self.src_len,
            end: self.src_len,
        })
    }
    /// Advance past the current (non-comment) token, skipping any comments before it.
    fn advance(&mut self) {
        if let Some(i) = self.cur_idx() {
            self.pos = i + 1;
        }
    }
    fn at(&self, t: &Tok) -> bool {
        self.cur()
            .is_some_and(|c| std::mem::discriminant(c) == std::mem::discriminant(t))
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn bump(&mut self) -> Option<Spanned> {
        let i = self.cur_idx()?;
        let s = self.toks[i].clone();
        self.pos = i + 1;
        Some(s)
    }

    /// Does the source between `from` and `to` contain a blank line (≥2 newlines)?
    fn blank_gap(&self, from: u32, to: u32) -> bool {
        self.src
            .get(from as usize..to as usize)
            .is_some_and(|g| g.matches('\n').count() >= 2)
    }

    /// End offset of the most recently consumed token (the clause's `.`).
    fn prev_end(&self) -> u32 {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.toks.get(i))
            .map(|s| s.span.end)
            .unwrap_or(self.src_len)
    }

    /// Consume consecutive comment tokens at `pos` and return their text.
    fn take_leading_comments(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(Spanned {
            tok: Tok::Comment(s),
            ..
        }) = self.toks.get(self.pos)
        {
            out.push(s.clone());
            self.pos += 1; // raw advance past a comment token
        }
        out
    }

    fn err_expected(&mut self, what: &str) {
        let found = self
            .cur()
            .map(|t| t.describe())
            .unwrap_or_else(|| "end of input".into());
        let code = if self.cur().is_none() {
            codes::PARSE_UNEXPECTED_EOF
        } else {
            codes::PARSE_EXPECTED
        };
        self.diags.error(
            code,
            format!("expected {what}, found {found}"),
            self.cur_span(),
        );
    }

    fn expect(&mut self, t: &Tok, what: &str) -> PResult<Span> {
        if self.at(t) {
            let span = self.cur_span();
            self.advance();
            Ok(span)
        } else {
            self.err_expected(what);
            Err(())
        }
    }

    fn expect_const(&mut self, what: &str) -> PResult<(String, Span)> {
        if let Some(Spanned {
            tok: Tok::Const(name),
            span,
        }) = self.cur_spanned()
        {
            let r = (name.clone(), *span);
            self.advance();
            Ok(r)
        } else {
            self.err_expected(what);
            Err(())
        }
    }

    /// Skip to just past the next `.` (or EOF) after a clause error.
    fn recover(&mut self) {
        while let Some(s) = self.bump() {
            if s.tok == Tok::Dot {
                break;
            }
        }
    }

    fn not_impl(&mut self, span: Span, what: &str) {
        self.diags.error(
            codes::NOT_IMPLEMENTED,
            format!("{what} is not implemented in Phase 0"),
            span,
        );
    }

    // --- items ---------------------------------------------------------------

    fn item(&mut self) -> PResult<Item> {
        self.vars.clear();
        match self.cur() {
            Some(Tok::Domain) => self.domain_decl(),
            Some(Tok::Pred) => self.pred_decl(),
            Some(Tok::Neural) => self.neural_decl(),
            Some(Tok::Input) => self.input_decl(),
            Some(Tok::Question | Tok::QProb | Tok::QGrad) => self.query(),
            Some(Tok::AtTerms | Tok::AtAsp) => self.pragma(),
            Some(Tok::IntLit(_) | Tok::Float(_)) => self.annotated_fact(),
            Some(Tok::Const(_)) => self.rule_or_fact(),
            _ => {
                self.err_expected("a declaration, rule, or fact");
                Err(())
            }
        }
    }

    fn domain_decl(&mut self) -> PResult<Item> {
        self.bump(); // `domain`
        let (name, _) = self.expect_const("a domain name")?;
        self.expect(&Tok::Dot, "`.`")?;
        Ok(Item::new(ItemKind::Domain(DomainDecl { name })))
    }

    fn pred_decl(&mut self) -> PResult<Item> {
        self.bump(); // `pred`
        let (name, _) = self.expect_const("a predicate name")?;
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                args.push(self.arg_type()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`)`")?;
        self.expect(&Tok::Colon, "`:`")?;
        let (annotation, ann_span) = self.annotation()?;
        let effects = self.effects();
        self.expect(&Tok::Dot, "`.`")?;
        if !annotation.is_phase0_executable() {
            self.not_impl(ann_span, "the Prov/Prov_k annotation");
        }
        Ok(Item::new(ItemKind::Predicate(PredDecl {
            name,
            sig: Signature {
                args,
                annotation,
                effects,
            },
            neural: None,
        })))
    }

    fn arg_type(&mut self) -> PResult<ArgType> {
        match self.cur() {
            Some(Tok::Int) => {
                self.bump();
                Ok(ArgType::Int)
            }
            Some(Tok::Term) => {
                self.bump();
                let (name, _) = self.expect_const("a term-type name")?;
                Ok(ArgType::Term { name })
            }
            Some(Tok::Const(_)) => {
                let (name, _) = self.expect_const("a domain")?;
                Ok(ArgType::Domain { name })
            }
            _ => {
                self.err_expected("an argument type (domain, `int`, or `term`)");
                Err(())
            }
        }
    }

    fn annotation(&mut self) -> PResult<(Annotation, Span)> {
        let span = self.cur_span();
        let ann = match self.cur() {
            Some(Tok::Bool) => Annotation::Bool,
            Some(Tok::Trop) => Annotation::Trop,
            Some(Tok::Prov) => Annotation::Prov,
            Some(Tok::ProvK) => Annotation::ProvK { k: 0 },
            _ => {
                self.err_expected("an annotation (`Bool`, `Trop`, `Prov`, `Prov_k`)");
                return Err(());
            }
        };
        self.advance();
        Ok((ann, span))
    }

    fn effects(&mut self) -> Effects {
        let mut e = Effects::default();
        loop {
            match self.cur() {
                Some(Tok::Total) => e.termination = Termination::Total,
                Some(Tok::Partial) => e.termination = Termination::Partial,
                Some(Tok::Complete) => e.completeness = Completeness::Complete,
                Some(Tok::SoundOnly) => e.completeness = Completeness::SoundOnly,
                Some(Tok::Deterministic) => e.determinism = Determinism::Deterministic,
                Some(Tok::Stochastic) => e.determinism = Determinism::Stochastic,
                _ => break,
            }
            self.advance();
        }
        e
    }

    fn neural_decl(&mut self) -> PResult<Item> {
        let kw = self.cur_span();
        self.bump(); // `neural`
        let (name, _) = self.expect_const("a predicate name")?;
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                let (dom, _) = self.expect_const("a domain")?;
                args.push(ArgType::Domain { name: dom });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`)`")?;
        self.expect(&Tok::From, "`from`")?;
        self.expect(&Tok::Model, "`model`")?;
        let model = self.expect_str("a model name")?;
        self.expect(&Tok::Dot, "`.`")?;
        let _ = kw;
        // A neural predicate is Bool-deductive; its ground atoms are the model's
        // soft (probabilistic) outputs, differentiated back through `?grad`.
        let effects = Effects {
            determinism: Determinism::Stochastic,
            ..Effects::default()
        };
        Ok(Item::new(ItemKind::Predicate(PredDecl {
            name,
            sig: Signature {
                args,
                annotation: Annotation::Bool,
                effects,
            },
            neural: Some(NeuralSpec { model }),
        })))
    }

    fn expect_str(&mut self, what: &str) -> PResult<String> {
        if let Some(Spanned {
            tok: Tok::Str(s), ..
        }) = self.cur_spanned()
        {
            let r = s.clone();
            self.advance();
            Ok(r)
        } else {
            self.err_expected(what);
            Err(())
        }
    }

    fn input_decl(&mut self) -> PResult<Item> {
        self.bump(); // `input`
        let (pred, _) = self.expect_const("a predicate name")?;
        self.expect(&Tok::From, "`from`")?;
        let path = self.expect_str("a TSV path")?;
        self.expect(&Tok::Dot, "`.`")?;
        Ok(Item::new(ItemKind::Input(InputDecl { pred, path })))
    }

    fn query(&mut self) -> PResult<Item> {
        let kind = match self.cur() {
            Some(Tok::Question) => QueryKind::Plain,
            Some(Tok::QProb) => QueryKind::Prob,
            Some(Tok::QGrad) => QueryKind::Grad,
            _ => unreachable!("query() entered without a query token"),
        };
        self.bump();
        let atom = self.atom()?;
        self.expect(&Tok::Dot, "`.`")?;
        // `?prob` and `?grad` both execute via режим B (Phase 4 + gradient wiring).
        Ok(Item::new(ItemKind::Query(Query { atom, kind })))
    }

    fn pragma(&mut self) -> PResult<Item> {
        let (pragma, span) = match self.cur() {
            Some(Tok::AtTerms) => (Pragma::Terms, self.cur_span()),
            Some(Tok::AtAsp) => (Pragma::Asp, self.cur_span()),
            _ => unreachable!(),
        };
        self.bump();
        self.expect(&Tok::Dot, "`.`")?;
        // @asp → reference solver (Phase 5); @terms → structural terms (spec §1.4).
        let _ = span;
        Ok(Item::new(ItemKind::Pragma(pragma)))
    }

    fn annotated_fact(&mut self) -> PResult<Item> {
        // `int :: atom` is a tropical weight; `float :: atom` is a probabilistic
        // fact (both executed — прогр B is Phase 4, no longer gated).
        let (weight, prob) = match self.cur_spanned() {
            Some(Spanned {
                tok: Tok::IntLit(n),
                ..
            }) => (Some(Weight::Finite(*n)), None),
            Some(Spanned {
                tok: Tok::Float(f), ..
            }) => (None, Some(*f)),
            _ => unreachable!(),
        };
        self.bump();
        self.expect(&Tok::DoubleColon, "`::`")?;
        let atom = self.atom()?;
        self.expect(&Tok::Dot, "`.`")?;
        Ok(Item::new(ItemKind::Fact(Fact { atom, weight, prob })))
    }

    fn rule_or_fact(&mut self) -> PResult<Item> {
        let head = self.atom()?;
        if self.eat(&Tok::ColonDash) {
            let mut body = Vec::new();
            loop {
                body.push(self.literal()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::Dot, "`.`")?;
            self.check_singletons();
            Ok(Item::new(ItemKind::Rule(strata_ir::high::Rule {
                head,
                body,
            })))
        } else {
            self.expect(&Tok::Dot, "`.`")?;
            Ok(Item::new(ItemKind::Fact(Fact {
                atom: head,
                weight: None,
                prob: None,
            })))
        }
    }

    fn literal(&mut self) -> PResult<Literal> {
        if self.eat(&Tok::Not) {
            Ok(Literal::Neg(self.atom()?))
        } else {
            Ok(Literal::Pos(self.atom()?))
        }
    }

    fn atom(&mut self) -> PResult<Atom> {
        let (pred, _) = self.expect_const("a predicate name")?;
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                args.push(self.term()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`)`")?;
        Ok(Atom { pred, args })
    }

    fn term(&mut self) -> PResult<Term> {
        match self.cur_spanned() {
            Some(Spanned {
                tok: Tok::Var(name),
                span,
            }) => {
                let (name, span) = (name.clone(), *span);
                self.vars.push((name.clone(), span));
                self.advance();
                Ok(Term::Var { name })
            }
            Some(Spanned {
                tok: Tok::Const(name),
                ..
            }) => {
                let name = name.clone();
                self.advance();
                // A constant immediately followed by `(` is a constructor term
                // `functor(args…)` (`@terms`, spec §1.4); args are terms, recursive.
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    if !self.at(&Tok::RParen) {
                        loop {
                            args.push(self.term()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(&Tok::RParen, "`)`")?;
                    Ok(Term::Compound {
                        functor: name,
                        args,
                    })
                } else {
                    Ok(Term::Const { name })
                }
            }
            Some(Spanned {
                tok: Tok::IntLit(n),
                ..
            }) => {
                let value = *n;
                self.advance();
                Ok(Term::Int { value })
            }
            Some(Spanned { tok, .. }) if agg_op(tok).is_some() => {
                let op = agg_op(tok).unwrap();
                self.advance();
                self.expect(&Tok::Lt, "`<`")?;
                let (var, span) = self.expect_var("an aggregate variable")?;
                self.vars.push((var.clone(), span)); // counts toward singleton analysis
                self.expect(&Tok::Gt, "`>`")?;
                Ok(Term::Agg { op, var })
            }
            _ => {
                self.err_expected("a term (variable, constant, integer, or aggregate)");
                Err(())
            }
        }
    }

    fn expect_var(&mut self, what: &str) -> PResult<(String, Span)> {
        if let Some(Spanned {
            tok: Tok::Var(name),
            span,
        }) = self.cur_spanned()
        {
            let r = (name.clone(), *span);
            self.advance();
            Ok(r)
        } else {
            self.err_expected(what);
            Err(())
        }
    }

    /// D3: a variable occurring exactly once in a rule is an error (not a
    /// warning); the fix is to prefix it with `_`.
    fn check_singletons(&mut self) {
        let vars = std::mem::take(&mut self.vars);
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (name, _) in &vars {
            *counts.entry(name.as_str()).or_default() += 1;
        }
        for (name, span) in &vars {
            if name.starts_with('_') {
                continue; // explicitly-ignored variable
            }
            if counts[name.as_str()] == 1 {
                self.diags.error_fix(
                    codes::SINGLETON_VAR,
                    format!("variable `{name}` appears only once in this rule"),
                    *span,
                    FixPatch {
                        span: *span,
                        replacement: format!("_{name}"),
                    },
                );
            }
        }
    }
}

fn agg_op(tok: &Tok) -> Option<AggOp> {
    match tok {
        Tok::Min => Some(AggOp::Min),
        Tok::Max => Some(AggOp::Max),
        Tok::Sum => Some(AggOp::Sum),
        Tok::Count => Some(AggOp::Count),
        Tok::ProbOr => Some(AggOp::ProbOr),
        _ => None,
    }
}
