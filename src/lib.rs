//! `ikigai-sexpr` — the neutral, language-agnostic **s-expression foundation** for
//! ikigai's portable-code arc.
//!
//! Three layers, in increasing dependency:
//!
//! 1. **The datum** — [`Sexpr`], a tiny s-expression AST (`Symbol`/`Str`/`Int`/`List`)
//!    with a recursive-descent [`parse`] reader and a round-tripping [`write`] printer.
//!    Pure Rust, no dependencies. Every lisp/reader adapts *into* this; the transreptor
//!    reads s-expr *text* with no lisp engine at all.
//! 2. **The compilers** — two **pure, kernel-free** total functions `&Sexpr → String`,
//!    sharing one term-rendering core (IRI validation + literal escaping):
//!    - [`sexpr_to_sparql`] turns a query-shaped s-expression into a SPARQL SELECT.
//!      Lifted (in logic) from `ikigai-lisp`'s Steel-coupled compiler, retyped onto
//!      [`Sexpr`] so it carries no eval/channel/capability concern.
//!    - [`sexpr_to_turtle`] turns a graph-shaped s-expression into RDF **Turtle**.
//!
//!    In both, string literals and IRIs are **escaped/validated, never interpolated** — a
//!    term can never break out and inject syntax — and any malformed input is a clear
//!    [`SexprError`], never a panic. Clauses may appear in any order; output is emitted in
//!    canonical order.
//! 3. **The endpoints** — two first-class `ik:Transreptor`s, the only layer that depends
//!    on `ikigai-core`: [`urn:sparql:from-sexpr`](space) (`text/x-sexpr` →
//!    `application/sparql-query`, requires a `(select …)`) and `urn:rdf:from-sexpr`
//!    (`text/x-sexpr` → `text/turtle`, requires a `(graph …)`). Both read the same
//!    `text/x-sexpr` input; they disambiguate by **output media type** *and* by the
//!    document's **head symbol** (`select` vs `graph`) — each errors on the wrong head.
//!
//! ## The accepted query grammar (`sexpr_to_sparql`)
//!
//! ```text
//! (select (?a ?b …) | *          ; projection: a var list, or * for all
//!   (prefix (rdf "http://…") …)   ; optional PREFIX lines
//!   (where (S P O) …)             ; triple patterns; S/P/O = ?var | (iri "…")
//!                                 ;   | pfx:local | "string" | integer | a
//!   (order-by ?v (desc ?w) …)     ; optional
//!   (limit N))                    ; optional
//! ```
//!
//! The head symbol must be `select`; an unknown head, a malformed clause, or an
//! unsupported term yields an `Err`. The emitted query always orders its parts
//! canonically: PREFIX → SELECT/WHERE → ORDER BY → LIMIT.
//!
//! ## The accepted graph grammar (`sexpr_to_turtle`)
//!
//! ```text
//! (graph
//!   (prefix (ex "http://example.org/") …)   ; optional PREFIX bindings (0 or more clauses)
//!   (ex:alice a foaf:Person)                ; each remaining form is one triple (S P O)
//!   (ex:alice foaf:name "Alice")            ;   S/P = pfx:local | (iri "…")
//!   (ex:alice foaf:age 42)                  ;   O   = S-terms | "string" | integer
//!   (ex:alice ex:score (lit "3.14" xsd:decimal))   ;       | (lit "v" pfx:dt) typed
//!   (ex:alice foaf:name (lit "Alix" @fr)))         ;       | (lit "v" @lang)  language-tagged
//! ```
//!
//! The head symbol must be `graph`. `a` in predicate position renders as `rdf:type` and
//! auto-binds the `rdf:` prefix. **No blank nodes** — a `_:x` form is rejected; every node
//! must be a stable IRI (skolemize). The emitted Turtle is canonical: `@prefix` lines, then
//! one `S P O .` statement per triple, each newline-terminated. It always parses as RDF.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, EndpointSpace, Error as CoreError, Exact, Invocation, ReprType,
    Representation, Result as CoreResult, Verb,
};
use oxrdf::{NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// The s-expression media type this crate reads: an s-expr *document* as UTF-8 text.
/// Human-readable and reader-neutral, so `text/x-*` (the conventional unregistered-text
/// space) fits. Both transreptors read it; each requires a particular top form — a
/// `(select …)` for [`urn:sparql:from-sexpr`](space), a `(graph …)` for `urn:rdf:from-sexpr`.
pub const MEDIA_SEXPR: &str = "text/x-sexpr";

/// The output media type of the query transreptor: the IANA-registered SPARQL query type.
pub const MEDIA_SPARQL_QUERY: &str = "application/sparql-query";

/// The output media type of the graph transreptor: RDF Turtle.
pub const MEDIA_TURTLE: &str = "text/turtle";

/// The XSD `string` datatype IRI — the `class` of the s-expression input.
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// The RDF namespace — auto-bound as the `rdf:` prefix when `a` (→ `rdf:type`) is used.
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

// --- slice 3c.2: the lossless code-as-graph vocabulary (self-contained in this crate) ---

/// The s-expression code-graph namespace. Defined and used ENTIRELY within this crate — it
/// is deliberately NOT part of the shared `ikigai-rs.dev/ns` vocabulary (no `/ns` deploy):
/// the encoding is still settling and has no second consumer yet. If a second consumer
/// appears, `sx:symbol`/`sx:root` are the terms to promote into the published vocab.
pub const SX_NS: &str = "https://ikigai-rs.dev/ns/sexpr#";

/// Datatype IRI marking a literal as a `Symbol` (distinct from `xsd:string`, so a symbol
/// round-trips as a symbol and never collapses into a string).
const SX_SYMBOL: &str = "https://ikigai-rs.dev/ns/sexpr#symbol";

/// Predicate on `<urn:sexpr:document>` naming the top node/literal of the encoded datum.
const SX_ROOT: &str = "https://ikigai-rs.dev/ns/sexpr#root";

/// The fixed document node whose `sx:root` names the encoded datum's top.
const SEXPR_DOCUMENT: &str = "urn:sexpr:document";

/// Prefix for content-addressed cons-cell node IRIs (`urn:sexpr:<hex>`).
const SEXPR_NODE_PREFIX: &str = "urn:sexpr:";

/// The distinguishing output profile of the `urn:sexpr:to-rdf` code-graph. Same base media
/// type as 3c's domain Turtle (`text/turtle`) but a distinct profile, so `urn:transrept:auto`
/// can tell the lossless-structure transreption apart from the domain-graph one.
const CODE_GRAPH_PROFILE: &str = "https://ikigai-rs.dev/ns/sexpr#code-graph";

/// `rdf:first` — a cons cell's head.
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
/// `rdf:rest` — a cons cell's tail (a node IRI, or `rdf:nil`).
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
/// `rdf:nil` — the empty list.
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
/// The XSD `integer` datatype IRI — an `Int` atom's literal type.
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// The output media type of the code-graph transreptor: Turtle, tagged with the code-graph
/// profile so it is self-describing and distinguishable from a domain graph.
pub const MEDIA_TURTLE_CODE_GRAPH: &str =
    "text/turtle; profile=\"https://ikigai-rs.dev/ns/sexpr#code-graph\"";

/// A guard against unbounded recursion when decoding a (possibly hostile) graph. The encoder
/// never nests this deep; a graph that does is rejected rather than overflowing the stack.
const MAX_DECODE_DEPTH: usize = 4096;

// =====================================================================================
// The datum — a neutral s-expression AST.
// =====================================================================================

/// A neutral s-expression datum. The reader produces it, the printer round-trips it, and
/// the compiler consumes it. Deliberately minimal: symbols carry `?var`/`pfx:local`/`a`/`*`
/// and other bare tokens; only these four cases are needed to express a SPARQL SELECT.
#[derive(Clone, Debug, PartialEq)]
pub enum Sexpr {
    /// A bare symbol: `select`, `where`, `?s`, `rdf:type`, `a`, `*`, …
    Symbol(String),
    /// A double-quoted string literal (its decoded contents, without the quotes).
    Str(String),
    /// An integer literal.
    Int(i64),
    /// A parenthesized list of forms.
    List(Vec<Sexpr>),
}

/// An error from this crate's pure layers — reader or compiler. Kernel-free, so the
/// datum/reader/printer/compiler carry no `ikigai-core` dependency. The endpoint maps it
/// onto a kernel [`Error::Endpoint`](ikigai_core::Error::Endpoint).
#[derive(Clone, Debug, PartialEq)]
pub enum SexprError {
    /// The reader could not tokenize/parse the input (e.g. unbalanced parens).
    Parse(String),
    /// The compiler rejected a well-formed s-expr that isn't a valid query.
    Compile(String),
}

impl std::fmt::Display for SexprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SexprError::Parse(m) => write!(f, "sexpr parse error: {m}"),
            SexprError::Compile(m) => write!(f, "sparql-from-sexpr: {m}"),
        }
    }
}

impl std::error::Error for SexprError {}

impl SexprError {
    /// The human-readable detail, without the surface label [`Display`](std::fmt::Display)
    /// prepends. Lets an endpoint compose its own prefix (e.g. `urn:rdf:from-sexpr: …`)
    /// without doubling a compiler-specific label.
    pub fn detail(&self) -> &str {
        match self {
            SexprError::Parse(m) | SexprError::Compile(m) => m,
        }
    }
}

/// The pure layers' result type.
pub type SexprResult<T> = std::result::Result<T, SexprError>;

// =====================================================================================
// The reader — recursive-descent s-expression parser.
// =====================================================================================

/// Parse exactly one top-level s-expression from `src`. Whitespace and `;` line comments
/// are skipped. Errors on an empty input, unbalanced parens, an unterminated string, or
/// trailing tokens after the first form (use [`parse_all`] for a stream of forms).
pub fn parse(src: &str) -> SexprResult<Sexpr> {
    let mut r = Reader::new(src);
    r.skip_trivia();
    let form = r
        .read_form()?
        .ok_or_else(|| SexprError::Parse("empty input: expected an s-expression".to_string()))?;
    r.skip_trivia();
    if r.peek().is_some() {
        return Err(SexprError::Parse(
            "unexpected trailing tokens after the first form".to_string(),
        ));
    }
    Ok(form)
}

/// Parse every top-level s-expression in `src` (zero or more forms).
pub fn parse_all(src: &str) -> SexprResult<Vec<Sexpr>> {
    let mut r = Reader::new(src);
    let mut forms = Vec::new();
    loop {
        r.skip_trivia();
        match r.read_form()? {
            Some(form) => forms.push(form),
            None => break,
        }
    }
    Ok(forms)
}

/// A char cursor over the source. Char-based (not byte-based) so multi-byte UTF-8 inside
/// string literals and symbols is handled without index gymnastics.
struct Reader {
    chars: Vec<char>,
    pos: usize,
}

impl Reader {
    fn new(src: &str) -> Self {
        Reader {
            chars: src.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Skip whitespace and `;` line comments.
    fn skip_trivia(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else if c == ';' {
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Read one form (assumes trivia already skipped). `Ok(None)` at end of input.
    fn read_form(&mut self) -> SexprResult<Option<Sexpr>> {
        match self.peek() {
            None => Ok(None),
            Some('(') => self.read_list().map(Some),
            Some(')') => Err(SexprError::Parse(
                "unexpected `)` — unbalanced parens".to_string(),
            )),
            Some('"') => self.read_string().map(Some),
            Some(_) => self.read_atom().map(Some),
        }
    }

    /// Read a `( … )` list. Errors if the closing paren is missing.
    fn read_list(&mut self) -> SexprResult<Sexpr> {
        self.pos += 1; // consume '('
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => {
                    return Err(SexprError::Parse(
                        "unexpected end of input: missing `)`".to_string(),
                    ))
                }
                Some(')') => {
                    self.pos += 1; // consume ')'
                    return Ok(Sexpr::List(items));
                }
                // peek is Some, so read_form yields Some — but stay total.
                Some(_) => {
                    if let Some(form) = self.read_form()? {
                        items.push(form);
                    }
                }
            }
        }
    }

    /// Read a `"…"` string literal, decoding `\\ \" \n \r \t` escapes (an unknown `\c`
    /// decodes to the literal `c`). Errors on an unterminated string.
    fn read_string(&mut self) -> SexprResult<Sexpr> {
        self.pos += 1; // consume opening '"'
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(SexprError::Parse("unterminated string literal".to_string())),
                Some('"') => {
                    self.pos += 1; // consume closing '"'
                    return Ok(Sexpr::Str(out));
                }
                Some('\\') => {
                    self.pos += 1;
                    match self.peek() {
                        None => {
                            return Err(SexprError::Parse(
                                "unterminated escape at end of string".to_string(),
                            ))
                        }
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some('\\') => out.push('\\'),
                        Some('"') => out.push('"'),
                        Some(other) => out.push(other), // unknown escape → the literal char
                    }
                    self.pos += 1;
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// Read a bare atom up to the next delimiter (whitespace, paren, quote, `;`, EOF).
    /// Classifies as [`Sexpr::Int`] when it parses as an `i64`, else [`Sexpr::Symbol`].
    fn read_atom(&mut self) -> SexprResult<Sexpr> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '(' | ')' | '"' | ';') {
                break;
            }
            self.pos += 1;
        }
        let token: String = self.chars[start..self.pos].iter().collect();
        if let Ok(n) = token.parse::<i64>() {
            Ok(Sexpr::Int(n))
        } else {
            Ok(Sexpr::Symbol(token))
        }
    }
}

// =====================================================================================
// The printer — round-trips a datum to text.
// =====================================================================================

/// Print a [`Sexpr`] as canonical s-expression text. `parse(&write(x)) == x` for any
/// datum the reader can produce (the reader never yields a symbol that the printer would
/// re-tokenize differently). String literals are escaped so the printed form re-reads to
/// the same contents.
pub fn write(sexpr: &Sexpr) -> String {
    let mut out = String::new();
    write_into(sexpr, &mut out);
    out
}

fn write_into(sexpr: &Sexpr, out: &mut String) {
    match sexpr {
        Sexpr::Symbol(s) => out.push_str(s),
        Sexpr::Int(n) => out.push_str(&n.to_string()),
        Sexpr::Str(s) => out.push_str(&render_string_literal(s)),
        Sexpr::List(items) => {
            out.push('(');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_into(item, out);
            }
            out.push(')');
        }
    }
}

// =====================================================================================
// The compiler — a pure `&Sexpr` → SPARQL `String`, kernel-free.
//
// Lifted (in logic) from ikigai-lisp's `sexpr_to_sparql(&SteelVal)`; only the input type
// changes. It is a total function of its s-expression input: string literals and IRIs are
// escaped/validated (nothing interpolated raw), and any malformed input is a clear
// [`SexprError`] — never a panic. Clauses may appear in any order; the emitted query is
// always ordered canonically (PREFIX → SELECT/WHERE → ORDER BY → LIMIT).
// =====================================================================================

/// Compile a query-shaped s-expression into a SPARQL query string.
///
/// The accepted grammar is documented on the [crate] docs. The head symbol must be
/// `select`; an unknown head, a malformed clause, or an unsupported term yields an `Err`.
pub fn sexpr_to_sparql(value: &Sexpr) -> SexprResult<String> {
    let items = list_items(value).ok_or_else(|| {
        compile_err("query must be a list, e.g. (select (?s ?p ?o) (where (?s ?p ?o)))")
    })?;
    let head = items
        .first()
        .and_then(|v| symbol_name(v))
        .ok_or_else(|| compile_err("query must start with a head symbol (e.g. `select`)"))?;
    match head {
        "select" => compile_select(&items[1..]),
        other => Err(compile_err(&format!(
            "unknown query head `{other}`; only `select` is supported"
        ))),
    }
}

/// Compile the tail of a `(select …)` form: `PROJECTION CLAUSE…`, where PROJECTION is a
/// var list or `*` and each CLAUSE is `where`/`prefix`/`limit`/`order-by`. Clauses may
/// appear in any order; the emitted query always orders them canonically
/// (PREFIX → SELECT/WHERE → ORDER BY → LIMIT).
fn compile_select(rest: &[&Sexpr]) -> SexprResult<String> {
    let projection = rest.first().ok_or_else(|| {
        compile_err("select needs a projection: (select (?a …) …) or (select * …)")
    })?;
    let proj = compile_projection(projection)?;

    let mut prefixes: Vec<String> = Vec::new();
    let mut where_triples: Vec<String> = Vec::new();
    let mut limit: Option<String> = None;
    let mut order_by: Option<String> = None;
    let mut saw_where = false;

    for clause in &rest[1..] {
        let citems = list_items(clause)
            .ok_or_else(|| compile_err("each select clause must be a list, e.g. (where …)"))?;
        let chead = citems.first().and_then(|v| symbol_name(v)).ok_or_else(|| {
            compile_err("a select clause must start with a keyword (where/prefix/limit/order-by)")
        })?;
        match chead {
            "where" => {
                saw_where = true;
                for triple in &citems[1..] {
                    where_triples.push(compile_triple(triple)?);
                }
            }
            "prefix" => {
                for binding in &citems[1..] {
                    prefixes.push(compile_prefix(binding)?);
                }
            }
            "limit" => limit = Some(compile_limit(&citems[1..])?),
            "order-by" => order_by = Some(compile_order_by(&citems[1..])?),
            other => {
                return Err(compile_err(&format!(
                    "unknown select clause `{other}`; expected where/prefix/limit/order-by"
                )))
            }
        }
    }
    if !saw_where {
        return Err(compile_err("select needs a (where …) clause"));
    }

    let mut out = String::new();
    for line in &prefixes {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("SELECT ");
    out.push_str(&proj);
    out.push_str(" WHERE {");
    for triple in &where_triples {
        out.push_str("\n  ");
        out.push_str(triple);
    }
    out.push_str("\n}");
    if let Some(order) = &order_by {
        out.push('\n');
        out.push_str(order);
    }
    if let Some(lim) = &limit {
        out.push('\n');
        out.push_str(lim);
    }
    Ok(out)
}

/// The projection: `*` (a lone symbol) or a non-empty list of `?variables`.
fn compile_projection(value: &Sexpr) -> SexprResult<String> {
    if symbol_name(value) == Some("*") {
        return Ok("*".to_string());
    }
    let vars = list_items(value)
        .ok_or_else(|| compile_err("projection must be a var list (?a ?b …) or *"))?;
    if vars.is_empty() {
        return Err(compile_err(
            "projection var list is empty; use * to select all",
        ));
    }
    let mut rendered = Vec::with_capacity(vars.len());
    for v in vars {
        rendered.push(render_var(v)?);
    }
    Ok(rendered.join(" "))
}

/// A single triple pattern `(S P O)` → `S P O .`. The predicate additionally accepts the
/// bare symbol `a` (SPARQL's `rdf:type` keyword).
fn compile_triple(value: &Sexpr) -> SexprResult<String> {
    let terms = list_items(value).ok_or_else(|| compile_err("a triple must be a list (S P O)"))?;
    if terms.len() != 3 {
        return Err(compile_err(
            "a triple must have exactly three terms (S P O)",
        ));
    }
    let s = render_term(terms[0])?;
    let p = render_predicate(terms[1])?;
    let o = render_term(terms[2])?;
    Ok(format!("{s} {p} {o} ."))
}

/// A `(prefix (pfx "http://…") …)` binding → a `PREFIX pfx: <http://…>` line. The name may
/// be written `pfx` or `pfx:` (the colon is added on emit).
fn compile_prefix(value: &Sexpr) -> SexprResult<String> {
    let items = list_items(value)
        .ok_or_else(|| compile_err("a prefix binding must be a list (pfx \"http://…\")"))?;
    if items.len() != 2 {
        return Err(compile_err("a prefix binding is (pfx \"http://…\")"));
    }
    let raw = symbol_name(items[0])
        .ok_or_else(|| compile_err("prefix name must be a symbol, e.g. rdf"))?;
    let pfx = raw.strip_suffix(':').unwrap_or(raw);
    if !valid_pn_prefix(pfx) {
        return Err(compile_err(&format!("invalid prefix name `{pfx}`")));
    }
    let iri = match items[1] {
        Sexpr::Str(s) => render_iri(s)?,
        _ => return Err(compile_err("a prefix namespace must be a string IRI")),
    };
    Ok(format!("PREFIX {pfx}: {iri}"))
}

/// A `(limit N)` modifier — one non-negative integer.
fn compile_limit(args: &[&Sexpr]) -> SexprResult<String> {
    if args.len() == 1 {
        match args[0] {
            Sexpr::Int(n) if *n >= 0 => return Ok(format!("LIMIT {n}")),
            Sexpr::Int(_) => return Err(compile_err("limit must be a non-negative integer")),
            _ => {}
        }
    }
    Err(compile_err(
        "limit takes a single non-negative integer, e.g. (limit 10)",
    ))
}

/// An `(order-by ?v (desc ?w) …)` modifier — one or more conditions.
fn compile_order_by(args: &[&Sexpr]) -> SexprResult<String> {
    if args.is_empty() {
        return Err(compile_err("order-by needs at least one ?variable"));
    }
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(render_order_condition(arg)?);
    }
    Ok(format!("ORDER BY {}", parts.join(" ")))
}

/// One ORDER BY condition: a bare `?variable`, or `(asc ?v)` / `(desc ?v)`.
fn render_order_condition(value: &Sexpr) -> SexprResult<String> {
    if let Some(s) = symbol_name(value) {
        return if is_var(s) {
            Ok(s.to_string())
        } else {
            Err(compile_err("order-by symbol must be a ?variable"))
        };
    }
    let items = list_items(value)
        .ok_or_else(|| compile_err("order-by term must be ?var or (asc ?var)/(desc ?var)"))?;
    let keyword = match items.first().and_then(|v| symbol_name(v)) {
        Some("asc") => "ASC",
        Some("desc") => "DESC",
        _ => return Err(compile_err("order-by direction must be asc or desc")),
    };
    if items.len() != 2 {
        return Err(compile_err("(asc …)/(desc …) takes one ?variable"));
    }
    let var = render_var(items[1])?;
    Ok(format!("{keyword}({var})"))
}

/// A term in subject/object position: `?var` | `(iri "…")` | `pfx:local` | string | integer.
fn render_term(value: &Sexpr) -> SexprResult<String> {
    match value {
        Sexpr::Symbol(s) => render_symbol_term(s),
        Sexpr::Str(s) => Ok(render_string_literal(s)),
        Sexpr::Int(n) => Ok(n.to_string()),
        Sexpr::List(items) => {
            let items: Vec<&Sexpr> = items.iter().collect();
            match items.first().and_then(|v| symbol_name(v)) {
                Some("iri") => render_iri_form(&items[1..]),
                Some(other) => Err(compile_err(&format!(
                    "unknown term form `({other} …)`; only (iri \"…\") is a compound term"
                ))),
                None => Err(compile_err(
                    "a compound term must start with a symbol, e.g. (iri \"…\")",
                )),
            }
        }
    }
}

/// A predicate term: like [`render_term`], but also accepting the bare `a` keyword.
fn render_predicate(value: &Sexpr) -> SexprResult<String> {
    if symbol_name(value) == Some("a") {
        return Ok("a".to_string());
    }
    render_term(value)
}

/// A symbol used as a term: either a `?variable` or a `pfx:local` prefixed name.
fn render_symbol_term(s: &str) -> SexprResult<String> {
    if is_var(s) || is_prefixed_name(s) {
        Ok(s.to_string())
    } else {
        Err(compile_err(&format!(
            "unrecognized term symbol `{s}`; use ?var or pfx:local (full IRIs go in (iri \"…\"))"
        )))
    }
}

/// A `?variable` term, validated (only `?` + word characters) to keep it injection-safe.
fn render_var(value: &Sexpr) -> SexprResult<String> {
    match symbol_name(value) {
        Some(s) if is_var(s) => Ok(s.to_string()),
        _ => Err(compile_err("expected a ?variable")),
    }
}

/// The single-argument body of an `(iri "…")` compound term → a validated `<…>` IRIREF.
fn render_iri_form(args: &[&Sexpr]) -> SexprResult<String> {
    if args.len() == 1 {
        if let Sexpr::Str(s) = args[0] {
            return render_iri(s);
        }
    }
    Err(compile_err(
        "(iri …) takes exactly one string, e.g. (iri \"http://…\")",
    ))
}

/// Wrap a validated IRI in `<…>`. Rejects any character illegal in a SPARQL IRIREF (angle
/// brackets, quotes, braces, backslash, control chars, whitespace) — so a term can never
/// break out of the IRIREF and inject query syntax.
fn render_iri(iri: &str) -> SexprResult<String> {
    if iri.is_empty() {
        return Err(compile_err("(iri \"\") is empty"));
    }
    if iri.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' | ' '
            )
    }) {
        return Err(compile_err(&format!(
            "IRI `{iri}` contains characters not allowed in a SPARQL IRIREF"
        )));
    }
    Ok(format!("<{iri}>"))
}

/// Render a Rust string as a SPARQL string literal, escaping the reserved characters so
/// the literal cannot terminate early or inject syntax. Also the printer's string form.
fn render_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The items of a list, or `None` if it is not a list.
fn list_items(value: &Sexpr) -> Option<Vec<&Sexpr>> {
    match value {
        Sexpr::List(items) => Some(items.iter().collect()),
        _ => None,
    }
}

/// The name of a symbol value, or `None` if it is not a symbol.
fn symbol_name(value: &Sexpr) -> Option<&str> {
    match value {
        Sexpr::Symbol(s) => Some(s.as_str()),
        _ => None,
    }
}

/// A SPARQL variable: `?` followed by one or more ASCII word characters.
fn is_var(s: &str) -> bool {
    let mut chars = s.chars();
    if chars.next() != Some('?') {
        return false;
    }
    let rest = chars.as_str();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A conservative prefixed name `pfx:local` — a valid prefix and a word-ish local part.
fn is_prefixed_name(s: &str) -> bool {
    match s.find(':') {
        Some(idx) => valid_pn_prefix(&s[..idx]) && valid_pn_local(&s[idx + 1..]),
        None => false,
    }
}

/// A conservative SPARQL prefix label: empty (the default prefix) or a letter then
/// letters/digits.
fn valid_pn_prefix(pfx: &str) -> bool {
    if pfx.is_empty() {
        return true;
    }
    let mut chars = pfx.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

/// A conservative SPARQL local name: word characters and hyphens (may be empty).
fn valid_pn_local(local: &str) -> bool {
    local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A compile-time error from either compiler — never a panic.
fn compile_err(msg: &str) -> SexprError {
    SexprError::Compile(msg.to_string())
}

// =====================================================================================
// The graph compiler — a pure `&Sexpr` → Turtle `String`, kernel-free.
//
// The graph-authoring surface: an s-expression naming a fixed schema (RDF's own data
// model) compiles to canonical Turtle. It shares the SPARQL compiler's term-rendering
// core — `render_iri` (IRIREF validation) and `render_string_literal` (literal escaping)
// — so IRIs and literals are escaped/validated, never interpolated. Any malformed input
// is a clear [`SexprError`]; every success is valid RDF. NO blank nodes: skolemize to a
// stable IRI (a `_:x` form is rejected).
// =====================================================================================

/// Compile a graph-shaped s-expression into an RDF **Turtle** document.
///
/// The accepted grammar is documented on the [crate] docs. The head symbol must be
/// `graph`; an unknown head, a malformed clause/triple, a blank-node form, or an
/// unsupported term yields an `Err`. Output is canonical: `@prefix` lines (in declaration
/// order, plus an auto-bound `rdf:` if `a`/`rdf:type` is used), then one newline-terminated
/// `S P O .` statement per triple.
pub fn sexpr_to_turtle(value: &Sexpr) -> SexprResult<String> {
    let items = list_items(value)
        .ok_or_else(|| compile_err("a graph must be a list, e.g. (graph (ex:s ex:p ex:o))"))?;
    let head = items
        .first()
        .and_then(|v| symbol_name(v))
        .ok_or_else(|| compile_err("a graph must start with a head symbol (`graph`)"))?;
    if head != "graph" {
        return Err(compile_err(&format!(
            "unknown graph head `{head}`; only `graph` is supported"
        )));
    }
    compile_graph(&items[1..])
}

/// Compile the body of a `(graph …)` form: zero or more `(prefix …)` clauses interleaved
/// with `(S P O)` triples. A form is a prefix clause iff its head symbol is `prefix`
/// (never a valid subject, so the dispatch is unambiguous); every other form is a triple.
fn compile_graph(rest: &[&Sexpr]) -> SexprResult<String> {
    let mut prefix_lines: Vec<String> = Vec::new();
    let mut declared_rdf = false;
    let mut triples: Vec<String> = Vec::new();
    let mut used_rdf_type = false;

    for form in rest {
        let fitems = list_items(form).ok_or_else(|| {
            compile_err("each graph form must be a list — a triple (S P O) or (prefix …)")
        })?;
        if fitems.first().and_then(|v| symbol_name(v)) == Some("prefix") {
            for binding in &fitems[1..] {
                let (pfx, line) = compile_turtle_prefix(binding)?;
                if pfx == "rdf" {
                    declared_rdf = true;
                }
                prefix_lines.push(line);
            }
        } else {
            triples.push(compile_graph_triple(&fitems, &mut used_rdf_type)?);
        }
    }

    // `a`/`rdf:type` was used but the author didn't declare `rdf:` — auto-bind it, so the
    // emitted Turtle is self-contained and parses.
    if used_rdf_type && !declared_rdf {
        prefix_lines.push(format!("@prefix rdf: <{RDF_NS}> ."));
    }

    let mut out = String::new();
    for line in &prefix_lines {
        out.push_str(line);
        out.push('\n');
    }
    for triple in &triples {
        out.push_str(triple);
        out.push('\n');
    }
    Ok(out)
}

/// A `(prefix (pfx "http://…"))` binding → the prefix label and a `@prefix pfx: <http://…> .`
/// line. The name may be written `pfx` or `pfx:` (the colon is added on emit).
fn compile_turtle_prefix(value: &Sexpr) -> SexprResult<(String, String)> {
    let items = list_items(value)
        .ok_or_else(|| compile_err("a prefix binding must be a list (pfx \"http://…\")"))?;
    if items.len() != 2 {
        return Err(compile_err("a prefix binding is (pfx \"http://…\")"));
    }
    let raw = symbol_name(items[0])
        .ok_or_else(|| compile_err("prefix name must be a symbol, e.g. ex"))?;
    let pfx = raw.strip_suffix(':').unwrap_or(raw);
    if !valid_pn_prefix(pfx) {
        return Err(compile_err(&format!("invalid prefix name `{pfx}`")));
    }
    let iri = match items[1] {
        Sexpr::Str(s) => render_iri(s)?,
        _ => return Err(compile_err("a prefix namespace must be a string IRI")),
    };
    Ok((pfx.to_string(), format!("@prefix {pfx}: {iri} .")))
}

/// A single triple `(S P O)` → `S P O .`. Sets `used_rdf_type` when the `a` predicate
/// keyword is encountered (so the caller can auto-bind `rdf:`).
fn compile_graph_triple(terms: &[&Sexpr], used_rdf_type: &mut bool) -> SexprResult<String> {
    if terms.len() != 3 {
        return Err(compile_err(
            "a triple must have exactly three terms (S P O)",
        ));
    }
    let s = render_graph_iri(terms[0], "subject")?;
    let p = render_graph_predicate(terms[1], used_rdf_type)?;
    let o = render_graph_object(terms[2])?;
    Ok(format!("{s} {p} {o} ."))
}

/// A node in IRI position (subject, or a non-`a` predicate): a `pfx:local` prefixed name or
/// an `(iri "…")` form. Variables, literals, and blank nodes are rejected — the last with a
/// pointed "skolemize" message. `role` names the position for the error text.
fn render_graph_iri(value: &Sexpr, role: &str) -> SexprResult<String> {
    match value {
        Sexpr::Symbol(s) => {
            if s.starts_with("_:") {
                return Err(compile_err(
                    "blank nodes are not allowed — skolemize to a stable IRI (pfx:local or (iri \"…\"))",
                ));
            }
            if is_var(s) {
                return Err(compile_err(&format!(
                    "a {role} must be an IRI, not the variable `{s}`"
                )));
            }
            if is_prefixed_name(s) {
                Ok(s.to_string())
            } else {
                Err(compile_err(&format!(
                    "unrecognized {role} `{s}`; use pfx:local or (iri \"…\")"
                )))
            }
        }
        Sexpr::List(items) => {
            let items: Vec<&Sexpr> = items.iter().collect();
            match items.first().and_then(|v| symbol_name(v)) {
                Some("iri") => render_iri_form(&items[1..]),
                Some(other) => Err(compile_err(&format!(
                    "unknown {role} form `({other} …)`; only (iri \"…\") is an IRI form"
                ))),
                None => Err(compile_err(&format!(
                    "a compound {role} must start with a symbol, e.g. (iri \"…\")"
                ))),
            }
        }
        _ => Err(compile_err(&format!(
            "a {role} must be an IRI (pfx:local or (iri \"…\")), not a literal"
        ))),
    }
}

/// A predicate node: the bare `a` keyword (→ `rdf:type`, flagging `used_rdf_type`) or an IRI.
fn render_graph_predicate(value: &Sexpr, used_rdf_type: &mut bool) -> SexprResult<String> {
    if symbol_name(value) == Some("a") {
        *used_rdf_type = true;
        return Ok("rdf:type".to_string());
    }
    render_graph_iri(value, "predicate")
}

/// An object node: an IRI (as a subject), or a literal — a bare `"string"`, an integer, or a
/// typed/language-tagged `(lit "v" pfx:dt)` / `(lit "v" @lang)` form.
fn render_graph_object(value: &Sexpr) -> SexprResult<String> {
    match value {
        Sexpr::Str(s) => Ok(render_string_literal(s)),
        Sexpr::Int(n) => Ok(n.to_string()),
        Sexpr::Symbol(_) => render_graph_iri(value, "object"),
        Sexpr::List(items) => {
            let items: Vec<&Sexpr> = items.iter().collect();
            match items.first().and_then(|v| symbol_name(v)) {
                Some("iri") => render_iri_form(&items[1..]),
                Some("lit") => render_lit_form(&items[1..]),
                Some(other) => Err(compile_err(&format!(
                    "unknown object form `({other} …)`; expected (iri \"…\") or (lit \"v\" dt)"
                ))),
                None => Err(compile_err(
                    "a compound object must start with a symbol, e.g. (iri \"…\") or (lit …)",
                )),
            }
        }
    }
}

/// A `(lit "value" pfx:datatype)` or `(lit "value" @lang)` body → a typed/lang Turtle
/// literal. The value is escaped (never interpolated); the datatype is a validated IRI
/// (`pfx:local` or `(iri "…")`), the language tag a validated BCP-47-ish `@subtag`.
fn render_lit_form(args: &[&Sexpr]) -> SexprResult<String> {
    if args.len() != 2 {
        return Err(compile_err(
            "(lit …) takes a value and a datatype/lang, e.g. (lit \"3.14\" xsd:decimal) or (lit \"hi\" @en)",
        ));
    }
    let value = match args[0] {
        Sexpr::Str(s) => render_string_literal(s),
        _ => return Err(compile_err("(lit …) value must be a string literal")),
    };
    match args[1] {
        // Language tag: a symbol `@subtag` (letters/digits/hyphen after the `@`).
        Sexpr::Symbol(tag) if tag.starts_with('@') => {
            let sub = &tag[1..];
            if sub.is_empty() || !sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(compile_err(&format!("invalid language tag `{tag}`")));
            }
            Ok(format!("{value}{tag}"))
        }
        // Datatype IRI: pfx:local or (iri "…").
        other => {
            let dt = render_graph_iri(other, "datatype")?;
            Ok(format!("{value}^^{dt}"))
        }
    }
}

// =====================================================================================
// Slice 3c.2 — the LOSSLESS code-as-graph codec: `&Sexpr` ⇄ RDF Turtle, kernel-free.
//
// Distinct from `sexpr_to_turtle`/`sexpr_to_sparql` (which INTERPRET a `(graph …)`/`(select
// …)` form as domain triples / a query). Here the sexpr's own STRUCTURE becomes a graph:
//   - a `List` is an `rdf:List` (`rdf:first`/`rdf:rest`/`rdf:nil`);
//   - each cons cell's IRI is `urn:sexpr:<hex>`, the hex of a deterministic, bottom-up hash
//     of its subtree — so equal sub-lists SHARE a node (structural dedup) and the IRI *is* a
//     content fingerprint (good for signing/caching). No blank nodes (skolemized).
//   - atoms are typed literals: `Symbol`→`^^sx:symbol`, `Str`→`^^xsd:string`, `Int`→`^^xsd:integer`;
//   - `<urn:sexpr:document> sx:root <top>` names the start (a literal for a top-level atom).
// `rdf_to_sexpr(sexpr_to_rdf(s)) == s` for every datum the reader can produce.
// =====================================================================================

/// Encode ANY [`Sexpr`] as a **lossless** RDF Turtle document (the code-as-graph). Pure,
/// kernel-free, and **deterministic**: the same datum always yields byte-identical Turtle
/// (statements are content-addressed and sorted). Equal sub-lists collapse to one shared,
/// content-addressed cons node. Round-trips exactly through [`rdf_to_sexpr`]. Total — never
/// fails for a valid datum (the `Result` mirrors the sibling compilers' signature).
pub fn sexpr_to_rdf(sexpr: &Sexpr) -> SexprResult<String> {
    let mut stmts: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let root = emit_node(sexpr, &mut stmts, &mut seen);

    // Content-address ⇒ traversal order is irrelevant; sort for byte-stable output.
    stmts.sort();

    let mut out = String::new();
    out.push_str(&format!("@prefix rdf: <{RDF_NS}> .\n"));
    out.push_str(&format!("@prefix sx: <{SX_NS}> .\n"));
    out.push_str(&format!("@prefix xsd: <{XSD_STRING_NS}> .\n"));
    out.push_str(&format!("<{SEXPR_DOCUMENT}> sx:root {root} .\n"));
    for s in &stmts {
        out.push_str(s);
        out.push('\n');
    }
    Ok(out)
}

/// The XSD namespace (for the `@prefix xsd:` line). `XSD_STRING`/`XSD_INTEGER` live under it.
const XSD_STRING_NS: &str = "http://www.w3.org/2001/XMLSchema#";

/// Emit the Turtle term denoting `sexpr` — a typed literal for an atom, or a node IRI /
/// `rdf:nil` for a list — appending each *new* cons cell's two statements to `stmts`. A cons
/// node already in `seen` is not re-emitted (that is where structural sharing shows up).
fn emit_node(sexpr: &Sexpr, stmts: &mut Vec<String>, seen: &mut HashSet<String>) -> String {
    match sexpr {
        Sexpr::Symbol(s) => typed_literal(s, "sx:symbol"),
        Sexpr::Str(s) => typed_literal(s, "xsd:string"),
        Sexpr::Int(n) => format!("\"{n}\"^^xsd:integer"),
        Sexpr::List(items) => emit_list(items, stmts, seen),
    }
}

/// Emit the cons chain for a list slice, returning its node term (`<urn:sexpr:…>` or, for the
/// empty list, `rdf:nil`). Content-addressed: the IRI is the hex of the subtree hash.
fn emit_list(items: &[Sexpr], stmts: &mut Vec<String>, seen: &mut HashSet<String>) -> String {
    if items.is_empty() {
        return "rdf:nil".to_string();
    }
    let iri = node_iri(items);
    let node = format!("<{iri}>");
    if seen.insert(iri) {
        let first = emit_node(&items[0], stmts, seen);
        let rest = emit_list(&items[1..], stmts, seen);
        stmts.push(format!("{node} rdf:first {first} ."));
        stmts.push(format!("{node} rdf:rest {rest} ."));
    }
    node
}

/// A typed Turtle literal `"escaped-value"^^dt` — reusing [`render_string_literal`] so the
/// lexical form is escaped, never interpolated (a symbol/string can't break out of the graph).
fn typed_literal(value: &str, datatype_qname: &str) -> String {
    format!("{}^^{datatype_qname}", render_string_literal(value))
}

/// The content-addressed IRI of a cons cell heading `items`.
fn node_iri(items: &[Sexpr]) -> String {
    format!("{SEXPR_NODE_PREFIX}{}", to_hex(&list_hash(items)))
}

/// The deterministic, domain-separated content hash of a datum's subtree (bottom-up). Equal
/// subtrees hash equally ⇒ shared, content-addressed nodes. No time/randomness — reproducible.
fn content_hash(sexpr: &Sexpr) -> [u8; 32] {
    match sexpr {
        Sexpr::Symbol(s) => atom_hash(0x01, s.as_bytes()),
        Sexpr::Str(s) => atom_hash(0x02, s.as_bytes()),
        Sexpr::Int(n) => atom_hash(0x03, &n.to_le_bytes()),
        Sexpr::List(items) => list_hash(items),
    }
}

/// Hash of a single atom: a kind tag, a length prefix, then the payload bytes (the length
/// prefix removes any concatenation ambiguity between differently-split payloads).
fn atom_hash(tag: u8, payload: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([tag]);
    h.update((payload.len() as u64).to_le_bytes());
    h.update(payload);
    h.finalize().into()
}

/// Hash of a cons chain starting at `items`: the empty chain is the `nil` marker; otherwise
/// `H(0x04 ‖ hash(first) ‖ hash(rest))` — so the whole subtree determines the node IRI.
fn list_hash(items: &[Sexpr]) -> [u8; 32] {
    let Some((first, rest)) = items.split_first() else {
        let mut h = Sha256::new();
        h.update([0x00]); // nil marker
        return h.finalize().into();
    };
    let fh = content_hash(first);
    let rh = list_hash(rest);
    let mut h = Sha256::new();
    h.update([0x04]); // cons marker
    h.update(fh);
    h.update(rh);
    h.finalize().into()
}

/// Lowercase hex of a byte slice (no dependency, deterministic).
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Decode a lossless code-graph (Turtle emitted by [`sexpr_to_rdf`], or any graph in the same
/// shape) back to the EXACT [`Sexpr`]. Reads `<urn:sexpr:document> sx:root`, walks the
/// `rdf:List`, and decodes atoms by datatype (`sx:symbol`→Symbol, `xsd:string`→Str,
/// `xsd:integer`→Int). Every malformed/ill-typed input is a clear [`SexprError::Parse`] —
/// never a panic. Other triples in the document are ignored, so a code-graph can be embedded
/// in a larger graph.
pub fn rdf_to_sexpr(turtle: &str) -> SexprResult<Sexpr> {
    let mut firsts: HashMap<String, RdfTerm> = HashMap::new();
    let mut rests: HashMap<String, RdfTerm> = HashMap::new();
    let mut root: Option<RdfTerm> = None;

    for quad in RdfParser::from_format(RdfFormat::Turtle).for_slice(turtle.as_bytes()) {
        let quad = quad.map_err(|e| SexprError::Parse(format!("turtle parse error: {e}")))?;
        let subject = match &quad.subject {
            NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            // Our code-graphs are skolemized — a blank-node subject can't be one of ours.
            NamedOrBlankNode::BlankNode(_) => continue,
        };
        match quad.predicate.as_str() {
            RDF_FIRST => {
                firsts.insert(subject, term_of(&quad.object)?);
            }
            RDF_REST => {
                rests.insert(subject, term_of(&quad.object)?);
            }
            SX_ROOT if subject == SEXPR_DOCUMENT => {
                root = Some(term_of(&quad.object)?);
            }
            _ => {} // ignore unrelated triples
        }
    }

    let root = root.ok_or_else(|| {
        SexprError::Parse(format!(
            "no `<{SEXPR_DOCUMENT}> sx:root …` triple — not an s-expression code-graph"
        ))
    })?;
    decode_term(&root, &firsts, &rests, 0)
}

/// A minimal owned RDF term — the only two shapes a code-graph uses in first/rest/root
/// position (an IRI or a typed literal). A blank node or quoted triple is out of shape.
enum RdfTerm {
    Iri(String),
    Literal { value: String, datatype: String },
}

/// Project an `oxrdf` [`Term`] into an [`RdfTerm`], rejecting shapes a code-graph never emits.
fn term_of(term: &Term) -> SexprResult<RdfTerm> {
    match term {
        Term::NamedNode(n) => Ok(RdfTerm::Iri(n.as_str().to_string())),
        Term::Literal(l) => Ok(RdfTerm::Literal {
            value: l.value().to_string(),
            datatype: l.datatype().as_str().to_string(),
        }),
        _ => Err(SexprError::Parse(
            "unsupported RDF term (blank node or quoted triple) in a code-graph".to_string(),
        )),
    }
}

/// Decode a term in root/first position: a literal → an atom; `rdf:nil` → the empty list; a
/// cons-node IRI → the list it heads.
fn decode_term(
    term: &RdfTerm,
    firsts: &HashMap<String, RdfTerm>,
    rests: &HashMap<String, RdfTerm>,
    depth: usize,
) -> SexprResult<Sexpr> {
    if depth > MAX_DECODE_DEPTH {
        return Err(SexprError::Parse(
            "code-graph nests deeper than the decode limit".to_string(),
        ));
    }
    match term {
        RdfTerm::Literal { value, datatype } => decode_atom(value, datatype),
        RdfTerm::Iri(iri) if iri == RDF_NIL => Ok(Sexpr::List(Vec::new())),
        RdfTerm::Iri(iri) => decode_list(iri, firsts, rests, depth),
    }
}

/// Decode a typed literal into the atom its datatype names.
fn decode_atom(value: &str, datatype: &str) -> SexprResult<Sexpr> {
    match datatype {
        SX_SYMBOL => Ok(Sexpr::Symbol(value.to_string())),
        XSD_STRING => Ok(Sexpr::Str(value.to_string())),
        XSD_INTEGER => value.parse::<i64>().map(Sexpr::Int).map_err(|_| {
            SexprError::Parse(format!(
                "`{value}` is not a valid xsd:integer for an Int atom"
            ))
        }),
        other => Err(SexprError::Parse(format!(
            "unknown atom datatype `{other}` (expected sx:symbol, xsd:string, or xsd:integer)"
        ))),
    }
}

/// Walk the cons chain from `head` (following `rdf:rest`) into a [`Sexpr::List`]. The chain is
/// followed iteratively (so a long list is cheap); a missing `rdf:first`/`rdf:rest`, a
/// literal tail, or a cycle is a clear error.
fn decode_list(
    head: &str,
    firsts: &HashMap<String, RdfTerm>,
    rests: &HashMap<String, RdfTerm>,
    depth: usize,
) -> SexprResult<Sexpr> {
    let mut items = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut cur = head.to_string();
    loop {
        if cur == RDF_NIL {
            break;
        }
        if !visited.insert(cur.clone()) {
            return Err(SexprError::Parse(format!(
                "cyclic rdf:rest chain at <{cur}> in code-graph"
            )));
        }
        let first = firsts
            .get(&cur)
            .ok_or_else(|| SexprError::Parse(format!("cons node <{cur}> has no rdf:first")))?;
        let rest = rests
            .get(&cur)
            .ok_or_else(|| SexprError::Parse(format!("cons node <{cur}> has no rdf:rest")))?;
        items.push(decode_term(first, firsts, rests, depth + 1)?);
        match rest {
            RdfTerm::Iri(next) => cur = next.clone(),
            RdfTerm::Literal { .. } => {
                return Err(SexprError::Parse(
                    "rdf:rest must be a node IRI or rdf:nil, not a literal".to_string(),
                ))
            }
        }
    }
    Ok(Sexpr::List(items))
}

// =====================================================================================
// The endpoints — `urn:sparql:from-sexpr` / `urn:rdf:from-sexpr`, the only
// ikigai-core-dependent layer.
// =====================================================================================

/// Mount the module at its conventional IRIs. A host links this crate and mounts the
/// returned space to give the kernel two language-agnostic transreptors: the query surface
/// `urn:sparql:from-sexpr` (`text/x-sexpr → application/sparql-query`) and the
/// graph-authoring surface `urn:rdf:from-sexpr` (`text/x-sexpr → text/turtle`).
pub fn space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:sparql:from-sexpr"), FromSexpr)
        .bind(Exact::new("urn:rdf:from-sexpr"), FromSexprTurtle)
        .bind(Exact::new("urn:sexpr:to-rdf"), ToRdf)
        .bind(Exact::new("urn:sexpr:from-rdf"), FromRdf)
}

/// The `urn:sparql:from-sexpr` transreptor: read an s-expr query TEXT (piped `content`, or
/// a named `in`), [`parse`] it, [`sexpr_to_sparql`] it, and emit the SPARQL string. A
/// first-class `ik:Transreptor` (`text/x-sexpr` → `application/sparql-query`) — no lisp
/// engine involved. Pure function of its input bytes, so its result is `.cacheable()`
/// (the kernel folds in the piped source's expiry down the pipe).
struct FromSexpr;

#[async_trait]
impl Endpoint for FromSexpr {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let src = read_source(inv, "urn:sparql:from-sexpr")?;
        let sexpr =
            parse(src).map_err(|e| CoreError::Endpoint(format!("urn:sparql:from-sexpr: {e}")))?;
        let sparql = sexpr_to_sparql(&sexpr)
            .map_err(|e| CoreError::Endpoint(format!("urn:sparql:from-sexpr: {e}")))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_SPARQL_QUERY).with_param("charset", "utf-8"),
            sparql.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sparql-from-sexpr"
    }

    fn describe(&self) -> Description {
        Description::new("sparql-from-sexpr")
            .title("SPARQL from s-expression")
            .summary(
                "Compile an s-expression SELECT into a SPARQL query — a language-agnostic \
                 transreptor with no lisp engine. Pipe an s-expr query in (or pass `in=`); the \
                 form is `(select (?vars…)|* (where (S P O)…) (prefix (pfx \"…\")…) (order-by …) \
                 (limit N))`. String literals and IRIs are escaped/validated, never \
                 interpolated; a malformed query is a clean error. Output is \
                 application/sparql-query — feed it to urn:sparql:select as `query=`.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the s-expression query TEXT to compile — usually piped in")
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(
                        "the s-expression query TEXT (positional/named alternative to content)",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_SPARQL_QUERY)
            // First-class `ik:Transreptor`: an s-expr query document → a SPARQL query.
            .transreptor([MEDIA_SEXPR], [MEDIA_SPARQL_QUERY])
    }
}

/// The `urn:rdf:from-sexpr` transreptor: read an s-expr graph TEXT (piped `content`, or a
/// named `in`), [`parse`] it, [`sexpr_to_turtle`] it, and emit the Turtle. A first-class
/// `ik:Transreptor` (`text/x-sexpr` → `text/turtle`) — no lisp engine involved. Shares the
/// `text/x-sexpr` input with [`FromSexpr`]; the two disambiguate by output media type and by
/// the document head (`graph` here, `select` there), so each errors on the wrong form. Pure
/// function of its input bytes, so its result is `.cacheable()`.
struct FromSexprTurtle;

#[async_trait]
impl Endpoint for FromSexprTurtle {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let src = read_source(inv, "urn:rdf:from-sexpr")?;
        let sexpr = parse(src)
            .map_err(|e| CoreError::Endpoint(format!("urn:rdf:from-sexpr: {}", e.detail())))?;
        let turtle = sexpr_to_turtle(&sexpr)
            .map_err(|e| CoreError::Endpoint(format!("urn:rdf:from-sexpr: {}", e.detail())))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_TURTLE).with_param("charset", "utf-8"),
            turtle.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "rdf-from-sexpr"
    }

    fn describe(&self) -> Description {
        Description::new("rdf-from-sexpr")
            .title("RDF (Turtle) from s-expression")
            .summary(
                "Compile an s-expression graph into RDF Turtle — a language-agnostic \
                 transreptor with no lisp engine. Pipe an s-expr graph in (or pass `in=`); the \
                 form is `(graph (prefix (pfx \"…\")…) (S P O)…)` where S/P are pfx:local or \
                 (iri \"…\") and O adds \"string\"/integer/(lit \"v\" dt) literals. `a` renders \
                 as rdf:type (rdf: auto-bound); NO blank nodes (skolemize). String literals and \
                 IRIs are escaped/validated, never interpolated; a malformed graph is a clean \
                 error. Output is text/turtle — feed it to urn:rdf:* to convert or store.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the s-expression graph TEXT to compile — usually piped in")
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(
                        "the s-expression graph TEXT (positional/named alternative to content)",
                    )
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_TURTLE)
            // First-class `ik:Transreptor`: an s-expr graph document → RDF Turtle.
            .transreptor([MEDIA_SEXPR], [MEDIA_TURTLE])
    }
}

/// The `urn:sexpr:to-rdf` transreptor: read an s-expr document TEXT (piped `content`, or a
/// named `in`), [`parse`] it, and [`sexpr_to_rdf`] it into the **lossless** code-graph
/// Turtle. A first-class `ik:Transreptor` (`text/x-sexpr` → `text/turtle` with the
/// **code-graph profile**). It shares `text/x-sexpr → text/turtle` with 3c's
/// `urn:rdf:from-sexpr`, but that endpoint INTERPRETS a `(graph …)` form as domain triples
/// while this one encodes the sexpr's STRUCTURE — the distinct output *profile* keeps the two
/// unambiguous for `urn:transrept:auto` (a request for plain domain Turtle selects
/// `urn:rdf:from-sexpr`; the code-graph is opt-in via the profile or explicit invocation).
/// Pure function of its input bytes, so `.cacheable()`.
struct ToRdf;

#[async_trait]
impl Endpoint for ToRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let src = read_source(inv, "urn:sexpr:to-rdf")?;
        let sexpr = parse(src)
            .map_err(|e| CoreError::Endpoint(format!("urn:sexpr:to-rdf: {}", e.detail())))?;
        let turtle = sexpr_to_rdf(&sexpr)
            .map_err(|e| CoreError::Endpoint(format!("urn:sexpr:to-rdf: {}", e.detail())))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_TURTLE)
                .with_param("charset", "utf-8")
                .with_param("profile", CODE_GRAPH_PROFILE),
            turtle.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-to-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-to-rdf")
            .title("Lossless RDF code-graph from an s-expression")
            .summary(
                "Encode ANY s-expression LOSSLESSLY as an RDF graph — its structure, so code \
                 becomes queryable/signable/routable. Pipe an s-expr in (or pass `in=`): a \
                 list → an rdf:List whose cons-cell IRIs are `urn:sexpr:<hash>` (content-\
                 addressed, so equal sub-lists share a node); atoms → typed literals \
                 (^^sx:symbol / ^^xsd:string / ^^xsd:integer); `<urn:sexpr:document> sx:root` \
                 names the top. Deterministic (byte-stable) and skolemized (no blank nodes). \
                 Output is Turtle with the code-graph profile; reverse with urn:sexpr:from-rdf. \
                 Distinct from urn:rdf:from-sexpr, which reads a (graph …) as domain triples.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the s-expression document TEXT to encode — usually piped in")
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary("the s-expression document TEXT (named alternative to content)")
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_TURTLE_CODE_GRAPH)
            // First-class `ik:Transreptor`: an s-expr document → its lossless code-graph.
            .transreptor([MEDIA_SEXPR], [MEDIA_TURTLE_CODE_GRAPH])
    }
}

/// The `urn:sexpr:from-rdf` transreptor: read a code-graph Turtle document (piped `content`,
/// or a named `in`), [`rdf_to_sexpr`] it, and [`write`] the reconstructed s-expression back
/// out. A first-class `ik:Transreptor` (`text/turtle` → `text/x-sexpr`) — the exact inverse
/// of [`ToRdf`]. Unambiguous: no other transreptor produces `text/x-sexpr` from `text/turtle`.
/// Pure function of its input bytes, so `.cacheable()`.
struct FromRdf;

#[async_trait]
impl Endpoint for FromRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let src = read_source(inv, "urn:sexpr:from-rdf")?;
        let sexpr = rdf_to_sexpr(src)
            .map_err(|e| CoreError::Endpoint(format!("urn:sexpr:from-rdf: {}", e.detail())))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_SEXPR).with_param("charset", "utf-8"),
            write(&sexpr).into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-from-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-from-rdf")
            .title("S-expression from a lossless RDF code-graph")
            .summary(
                "Reconstruct the EXACT s-expression from a lossless code-graph (the inverse of \
                 urn:sexpr:to-rdf). Pipe the Turtle in (or pass `in=`): reads \
                 `<urn:sexpr:document> sx:root`, walks the rdf:List, decodes atoms by datatype \
                 (sx:symbol→symbol, xsd:string→string, xsd:integer→integer). Malformed or \
                 ill-typed input is a clean error. Output is text/x-sexpr.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the code-graph Turtle TEXT to decode — usually piped in")
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary("the code-graph Turtle TEXT (named alternative to content)")
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_SEXPR)
            // First-class `ik:Transreptor`: a lossless code-graph → its s-expression.
            .transreptor([MEDIA_TURTLE], [MEDIA_SEXPR])
    }
}

/// The s-expr source: piped `content` (the transreptor/pipeline convention — a stage piped
/// into a from-sexpr transreptor arrives as `content`), falling back to a named `in`. `iri`
/// names the endpoint for the "no input" error.
fn read_source<'a>(inv: &'a Invocation<'_>, iri: &str) -> CoreResult<&'a str> {
    match inv.inline_str("content") {
        Ok(src) => Ok(src),
        Err(_) => inv.inline_str("in").map_err(|_| {
            CoreError::Endpoint(format!(
                "{iri} needs an s-expr document — pipe one in (e.g. \
                 `source <sexpr> | {iri}`) or pass `in=…`"
            ))
        }),
    }
}

#[cfg(test)]
mod tests;
