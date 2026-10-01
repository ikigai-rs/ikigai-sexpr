//! **Arrangements as s-expressions** — a kernel's arrangement written as a file.
//!
//! A kernel's arrangement is a [`Topology`] tree: endpoint spaces with their ordered doors,
//! fallbacks, mounts, aliases, limiters and levels. Core renders it to Turtle
//! ([`Topology::to_turtle`]), reads it back ([`Topology::from_turtle`]), and
//! [`build`](ikigai_core::build)s a live space from it. This module is the s-expression
//! surface for the same tree: `*.arrangement` files, media type
//! [`text/x-ikigai-arrangement`](MEDIA_ARRANGEMENT), going THROUGH core's `Topology` in both
//! directions — s-expression ↔ `Topology` here, `Topology` ↔ Turtle in core.
//!
//! ## The grammar
//!
//! A document is ONE space form. Anonymous spaces are nesting; a space that claims an IRI
//! says so with `:id`. Options are `:key value` pairs and may appear anywhere after the
//! head; the canonical printer puts them right after the positional parts.
//!
//! ```text
//! space := (endpoints [:id "iri"] door…)            ; ik:EndpointSpace, doors in order
//!        | (fallback [:id "iri"] space…)            ; ik:Fallback, layers in order
//!        | (mount "prefix" [:id "iri"] space)       ; ik:Mount, a LOCAL mount
//!        | (alias [:id "iri"] [:max-hops n] rule… space)          ; ik:Alias
//!        | (limit "family" [:id "iri"] [:match prefix|exact|template])   ; ik:Limit
//!        | (level "iri" [:seals ("prefix"…)] [:namespace "prefix"] space) ; ik:Level
//!        | (ref "iri")                               ; a named space declared EARLIER
//! door  := (door "pattern" endpoint [:match exact|template] [:confined space])
//! rule  := (exact "from" "to") | (prefix "from" "to")
//! ```
//!
//! - **The common case is short.** A door's match kind is inferred from its pattern — a `{`
//!   means a URI template, anything else an exact name — and a limiter's the same way, with
//!   a prefix as the default. `:match` is written only when the inference is wrong (a
//!   template with no variables, say). An alias's `:max-hops` is written only when it is not
//!   core's [`DEFAULT_MAX_HOPS`].
//! - **An endpoint is named, never minted.** `endpoint` is the name the host registered it
//!   under ([`Endpoint::name`]): a bare symbol, or a string when
//!   the name is not a plain symbol.
//! - **A name is a claim.** A named space met twice is ONE space, as a shared `Arc` is in
//!   code; the second place writes `(ref "iri")`. Restating it in full is accepted when it
//!   is identical, and refused when it differs.
//! - **A confinement lives at a door.** `:confined` takes the corridor the door's endpoint
//!   is confined to, and the corridor is named (its `:id` is the confinement's name).
//! - **An alias's rules are a table, not a list.** Core's `AliasTable` consults them most
//!   specific first whatever order they were added in (the longest name first, an exact rule
//!   ahead of a prefix one), so they are read, and printed, in that order. Doors and layers
//!   are the opposite: their order is meaning (first match wins), and it is kept exactly.
//!
//! ## What is refused, and why
//!
//! The grammar is exactly the arrangements core can [`build`](ikigai_core::build). Everything
//! else is refused, never skipped, with an error that says where ([`ArrangementError::at`])
//! and why: an unknown form or option; a kind core refuses (`opaque`, `rewrite`, `chain`,
//! `confine` where a space belongs); a door that matches by prefix or by a custom grammar; a
//! template that does not parse; a part missing, extra, or stated twice; an `:id` under
//! `urn:ikigai:space:_:` (where anonymous spaces are numbered); a seal stated twice (the
//! Turtle would keep it once); and two different arrangements under one name.
//!
//! ## What "lossless" means here
//!
//! Both transreptors are declared lossless, and it is a checked property, not a claim: each
//! conversion reads its own output back and refuses to answer if the arrangement it gets is
//! not the one it was given. What is kept is everything the arrangement MEANS — every node,
//! its identity, and every order. What the Turtle cannot keep is presentation: **a comment in
//! the source is the one thing the round trip cannot keep**, along with layout and the choice
//! between equivalent spellings (a string or a symbol for an endpoint name, an option's
//! position, an inferred `:match` written out, a restated space instead of a `ref`, alias
//! rules written out of table order). Printed back, an arrangement comes out in the canonical
//! form [`topology_to_arrangement`] writes.
//!
//! ## Bounds: core's, in core's units
//!
//! An arrangement is held to the bounds core reads and builds a declaration within
//! ([`MAX_DECLARATION_DEPTH`], [`MAX_DECLARATION_NODES`], [`MAX_DECLARATION_TEXT`]), counted
//! the way core counts them, and a document past one is refused with core's own typed
//! refusal, [`DeclarationError::TooLarge`], in [`ArrangementError::declaration`]:
//!
//! - **depth**: the root space is 1, and every space it encloses — a layer, the space a
//!   mount, an alias or a level encloses, a door's confined corridor — is one deeper (a door
//!   is not a level of its own);
//! - **nodes**: every space, every door and every alias rule, a named space counted again at
//!   every place it is used — so a `ref` costs everything it stands for;
//! - **text**: the bytes those nodes carry (every IRI a space claims, every pattern, endpoint
//!   name, prefix, family, rule, seal and namespace), counted the same way.
//!
//! A Turtle document is core's to refuse: [`Topology::from_turtle`] counts as it reads, so a
//! small document that would expand exponentially is refused after the bound, not after the
//! expansion. The s-expression reader counts in the same units AS IT READS, for the same
//! reason (a `ref` is one line of text and a whole subtree of the tree), and names the node
//! the way core would — a space's own IRI, an anonymous one numbered in pre-order under
//! `urn:ikigai:space:_:`, and anything inside a named space placed again named as that space
//! — so the two paths refuse an arrangement alike. The one difference: at a `ref`, depth is
//! checked before size, where core, walking the expanded subtree, meets whichever bound its
//! pre-order reaches first. The text itself first goes through the crate's bounded [`parse`]
//! reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, DeclarationBound, DeclarationError, Description, Door, Endpoint, EndpointSpace,
    Error as CoreError, Exact, Invocation, Iri, MatchKind, ReprType, Representation,
    Result as CoreResult, RuleKind, SpaceKind, Topology, TopologyRule, UriTemplate, Verb,
    DEFAULT_MAX_HOPS, MAX_DECLARATION_DEPTH, MAX_DECLARATION_NODES, MAX_DECLARATION_TEXT,
};

use crate::{
    content_summary, in_summary, parse, read_source, render_string_literal, Sexpr, MEDIA_TURTLE,
    XSD_STRING,
};

/// The media type of an arrangement written as an s-expression: an `*.arrangement` file.
/// DISTINCT from [`text/x-sexpr`](crate::MEDIA_SEXPR) on purpose: `urn:sexpr:to-rdf` already
/// transrepts `text/x-sexpr` to Turtle losslessly (and `urn:rdf:from-sexpr` with the caller's
/// consent), so a selector over that type could pick one of them and hand the builder a list
/// graph or a domain graph.
pub const MEDIA_ARRANGEMENT: &str = "text/x-ikigai-arrangement";

/// What this crate bounded an arrangement's depth by in 0.1.4: now core's bound, in core's
/// units, which count spaces and not doors.
#[deprecated(
    since = "0.1.5",
    note = "an arrangement is held to core's declaration bounds, in core's units: use \
            `ikigai_core::MAX_DECLARATION_DEPTH`, which counts spaces (a door is not a level)"
)]
pub const MAX_ARRANGEMENT_DEPTH: usize = MAX_DECLARATION_DEPTH;

/// What this crate bounded an arrangement's size by in 0.1.4: now core's bound, in core's
/// units, which count alias rules as well as spaces and doors.
#[deprecated(
    since = "0.1.5",
    note = "an arrangement is held to core's declaration bounds, in core's units: use \
            `ikigai_core::MAX_DECLARATION_NODES` (spaces, doors and alias rules) and \
            `ikigai_core::MAX_DECLARATION_TEXT`"
)]
pub const MAX_ARRANGEMENT_NODES: usize = MAX_DECLARATION_NODES;

/// Where core numbers anonymous nodes: a node IRI under it reads back as anonymous, so an
/// `:id` there would not survive the round trip. Core keeps the constant crate-private; the
/// prefix is part of the Turtle it writes (`Topology::to_turtle`), and the name a bound's
/// refusal gives an anonymous node.
const SKOLEM_PREFIX: &str = "urn:ikigai:space:_:";

/// **Mount the arrangement surface alone**: `urn:sexpr:arrangement-to-rdf` and
/// `urn:sexpr:arrangement-from-rdf`, and nothing else. What a host that reads `*.arrangement`
/// files needs, and all a page in a browser should pay for: with the crate's default `full`
/// feature off, nothing else in the crate is compiled.
///
/// ```
/// use std::sync::Arc;
/// use ikigai_core::{select_transreptor, Space};
/// use ikigai_sexpr::{arrangement_space, MEDIA_ARRANGEMENT, MEDIA_TURTLE};
///
/// let space: Arc<dyn Space> = Arc::new(arrangement_space());
/// let plan = select_transreptor(space.as_ref(), MEDIA_ARRANGEMENT, MEDIA_TURTLE).unwrap();
/// assert_eq!(plan[0].endpoint, "urn:sexpr:arrangement-to-rdf");
/// assert!(plan[0].lossless);
/// ```
pub fn arrangement_space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:sexpr:arrangement-to-rdf"), ArrangementToRdf)
        .bind(
            Exact::new("urn:sexpr:arrangement-from-rdf"),
            ArrangementFromRdf,
        )
}

/// Why an arrangement was refused: where, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArrangementError {
    /// Where the problem is: a path from the root, outermost first
    /// (`root (alias) › space (fallback) › layer 1 (endpoints <urn:x>) › door 3`), or a
    /// node's IRI as the Turtle names it (`<urn:ikigai:space:_:2>`). Empty when the problem is
    /// the document as a whole.
    pub at: String,
    /// What is wrong.
    pub reason: String,
    /// Core's own typed refusal, when the arrangement was refused by a rule of core's: a
    /// [`DeclarationError::TooLarge`] past one of the declaration bounds (from either path —
    /// the s-expression reader counts in core's units and refuses with core's error), or
    /// whatever [`Topology::from_turtle`] refused a Turtle document for. `None` when the
    /// refusal is this surface's own (its grammar, a name claimed twice). `reason` is then
    /// that error's text.
    ///
    /// ```
    /// use ikigai_core::{DeclarationBound, DeclarationError, MAX_DECLARATION_DEPTH};
    /// use ikigai_sexpr::arrangement_to_topology;
    ///
    /// // One fallback more than the bound allows, every one anonymous: core numbers the
    /// // 49th `urn:ikigai:space:_:49`, and so does the reader.
    /// let deep = format!(
    ///     "{}(limit \"urn:x:\"){}",
    ///     "(fallback ".repeat(MAX_DECLARATION_DEPTH),
    ///     ")".repeat(MAX_DECLARATION_DEPTH)
    /// );
    /// let refused = arrangement_to_topology(&deep).unwrap_err();
    /// assert_eq!(
    ///     refused.declaration.as_deref(),
    ///     Some(&DeclarationError::TooLarge {
    ///         bound: DeclarationBound::Depth,
    ///         limit: 48,
    ///         node: "urn:ikigai:space:_:49".into(),
    ///     })
    /// );
    /// ```
    pub declaration: Option<Box<DeclarationError>>,
}

impl ArrangementError {
    fn new(at: impl Into<String>, reason: impl Into<String>) -> Self {
        ArrangementError {
            at: at.into(),
            reason: reason.into(),
            declaration: None,
        }
    }

    /// Core's refusal `error`, found at `at`.
    fn core(at: impl Into<String>, error: Box<DeclarationError>) -> Self {
        ArrangementError {
            at: at.into(),
            reason: error.to_string(),
            declaration: Some(error),
        }
    }

    fn document(reason: impl Into<String>) -> Self {
        ArrangementError::new("", reason)
    }
}

impl fmt::Display for ArrangementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() {
            write!(f, "{}", self.reason)
        } else {
            write!(f, "at {}: {}", self.at, self.reason)
        }
    }
}

impl std::error::Error for ArrangementError {}

type Result<T> = std::result::Result<T, ArrangementError>;

// =====================================================================================
// s-expression → Topology
// =====================================================================================

/// **Read an arrangement**: s-expression text → the [`Topology`] it describes. The text goes
/// through the bounded [`parse`] reader; see the [module docs](self) for the grammar.
///
/// ```
/// use ikigai_sexpr::arrangement_to_topology;
///
/// let tree = arrangement_to_topology(
///     r#"(mount "urn:mod:" (endpoints (door "urn:mod:{name}" module)))"#,
/// )
/// .unwrap();
/// assert_eq!(tree.children.len(), 1);
///
/// let refused = arrangement_to_topology(r#"(tunnel "urn:x")"#).unwrap_err();
/// assert_eq!(refused.at, "root (tunnel)");
/// assert!(refused.reason.contains("unknown form"));
/// ```
pub fn arrangement_to_topology(src: &str) -> Result<Topology> {
    let form = parse(src).map_err(|e| ArrangementError::document(e.detail()))?;
    Reader::default().space(&form, "root", 1, None)
}

/// A named space already read: the tree, and what it costs in core's units each time it is
/// placed again — its nodes, its text, and how many spaces deep it goes.
struct Claimed {
    tree: Topology,
    nodes: usize,
    text: usize,
    height: usize,
}

#[derive(Default)]
struct Reader {
    /// Every named space read so far, by IRI.
    named: BTreeMap<String, Claimed>,
    /// The arrangement's size so far, as core counts it.
    tally: Tally,
}

// =====================================================================================
// The bounds, counted as core counts them
// =====================================================================================

/// One node's own share of the bounds, as core charges it: itself, its doors and its alias
/// rules, and the text they carry — never its children or its doors' corridors, which are
/// nodes of their own. Mirrors core's private `Budget::charge` (ikigai-core 0.1.84,
/// `declare.rs`); `tests/arrangement.rs` holds the two to the same refusal on every bound.
fn share(id: Option<&Iri>, kind: &SpaceKind) -> (usize, usize) {
    let mut nodes = 1;
    let mut text = id.map_or(0, |id| id.as_str().len());
    match kind {
        SpaceKind::EndpointSpace { doors } => {
            nodes += doors.len();
            text += doors
                .iter()
                .map(|d| d.pattern.len() + d.endpoint.len())
                .sum::<usize>();
        }
        SpaceKind::Alias { rules, .. } => {
            nodes += rules.len();
            text += rules
                .iter()
                .map(|r| r.from.len() + r.to.len())
                .sum::<usize>();
        }
        SpaceKind::Mount { prefix } => text += prefix.len(),
        SpaceKind::Limit { family, .. } => text += family.len(),
        SpaceKind::Level { seals, namespace } => {
            text += seals.iter().map(String::len).sum::<usize>();
            text += namespace.as_ref().map_or(0, String::len);
        }
        _ => {}
    }
    (nodes, text)
}

/// A whole tree's cost in core's units: nodes, text, and depth (the root is 1). Recursive;
/// only ever called on a tree the reader has already bounded.
fn size(tree: &Topology) -> (usize, usize, usize) {
    let (mut nodes, mut text) = share(tree.id.as_ref(), &tree.kind);
    let mut below = 0;
    let corridors = match &tree.kind {
        SpaceKind::EndpointSpace { doors } => {
            doors.iter().filter_map(|d| d.confined.as_deref()).collect()
        }
        _ => Vec::new(),
    };
    for inner in tree.children.iter().chain(corridors) {
        let (n, t, h) = size(inner);
        nodes = nodes.saturating_add(n);
        text = text.saturating_add(t);
        below = below.max(h);
    }
    (nodes, text, below + 1)
}

/// The running count of an arrangement against core's bounds, meeting nodes in the order
/// core's own measure does (pre-order: a node, then its children, then its doors'
/// corridors in door order) so that a refusal names the node core would name.
#[derive(Default)]
struct Tally {
    /// Every name met so far: a name met again is a space placed again, and everything under
    /// it is charged to that name.
    seen: BTreeSet<String>,
    /// Anonymous spaces met so far, outside any space placed again: the next one's number.
    skolem: usize,
    nodes: usize,
    text: usize,
}

impl Tally {
    /// Meet one node at `depth`, `again` naming the space placed again it lies under (if
    /// any): name it, check its depth, and charge its own share. Returns the name its
    /// descendants are charged under — `Some` inside a space placed again.
    fn meet(
        &mut self,
        id: Option<&Iri>,
        kind: &SpaceKind,
        depth: usize,
        again: Option<&str>,
    ) -> std::result::Result<Option<String>, Box<DeclarationError>> {
        let (me, below) = match (again, id) {
            (Some(outer), _) => (outer.to_string(), Some(outer.to_string())),
            (None, Some(id)) => {
                let me = id.as_str().to_string();
                if self.seen.insert(me.clone()) {
                    (me, None)
                } else {
                    (me.clone(), Some(me))
                }
            }
            (None, None) => {
                self.skolem += 1;
                (format!("{SKOLEM_PREFIX}{}", self.skolem), None)
            }
        };
        if depth > MAX_DECLARATION_DEPTH {
            return Err(too_large(DeclarationBound::Depth, &me));
        }
        let (nodes, text) = share(id, kind);
        self.charge(nodes, text, &me)?;
        Ok(below)
    }

    /// Place a named space again, at `depth`: everything it holds is charged again, to `me`.
    fn again(
        &mut self,
        claimed: &Claimed,
        depth: usize,
        me: &str,
    ) -> std::result::Result<(), Box<DeclarationError>> {
        if depth - 1 + claimed.height > MAX_DECLARATION_DEPTH {
            return Err(too_large(DeclarationBound::Depth, me));
        }
        self.charge(claimed.nodes, claimed.text, me)
    }

    fn charge(
        &mut self,
        nodes: usize,
        text: usize,
        me: &str,
    ) -> std::result::Result<(), Box<DeclarationError>> {
        self.nodes = self.nodes.saturating_add(nodes);
        self.text = self.text.saturating_add(text);
        if self.nodes > MAX_DECLARATION_NODES {
            Err(too_large(DeclarationBound::Nodes, me))
        } else if self.text > MAX_DECLARATION_TEXT {
            Err(too_large(DeclarationBound::Text, me))
        } else {
            Ok(())
        }
    }
}

/// Core's refusal for a declaration past `bound`, at the node `node`.
fn too_large(bound: DeclarationBound, node: &str) -> Box<DeclarationError> {
    Box::new(DeclarationError::TooLarge {
        bound,
        limit: bound.limit(),
        node: node.to_string(),
    })
}

/// Check a tree the caller built against core's bounds WITHOUT recursing — an explicit stack,
/// so a tree far deeper than the bound is refused rather than overflowing the check. The
/// same walk as core's private `measure` (which `build` runs), so a tree passes here exactly
/// when core would build it. Core does not export that check; this copy is the gap.
fn check(tree: &Topology) -> Result<()> {
    let mut tally = Tally::default();
    // (node, its depth, the space placed again that it lies under, if any)
    let mut stack: Vec<(&Topology, usize, Option<String>)> = vec![(tree, 1, None)];
    while let Some((node, depth, again)) = stack.pop() {
        let below = tally
            .meet(node.id.as_ref(), &node.kind, depth, again.as_deref())
            .map_err(from_declaration)?;
        // Pushed in reverse, so they pop in core's order: the children, then each door's
        // corridor in door order.
        if let SpaceKind::EndpointSpace { doors } = &node.kind {
            for corridor in doors.iter().rev().filter_map(|d| d.confined.as_deref()) {
                stack.push((corridor, depth + 1, below.clone()));
            }
        }
        for child in node.children.iter().rev() {
            stack.push((child, depth + 1, below.clone()));
        }
    }
    Ok(())
}

/// A form taken apart: its head, its positional parts in order, and its `:key value`
/// options.
struct Parts<'a> {
    head: &'a str,
    args: Vec<&'a Sexpr>,
    options: BTreeMap<&'a str, &'a Sexpr>,
}

impl<'a> Parts<'a> {
    /// Refuse an option this form does not take.
    fn allow(&self, at: &str, allowed: &[&str]) -> Result<()> {
        for key in self.options.keys() {
            if !allowed.contains(key) {
                let takes = if allowed.is_empty() {
                    "takes no options".to_string()
                } else {
                    format!(
                        "takes {}",
                        allowed
                            .iter()
                            .map(|k| format!("`:{k}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                return Err(ArrangementError::new(
                    at,
                    format!("unknown option `:{key}`: ({} …) {takes}", self.head),
                ));
            }
        }
        Ok(())
    }

    fn option(&self, key: &str) -> Option<&'a Sexpr> {
        self.options.get(key).copied()
    }
}

/// Take a form apart. Refuses anything that is not `(symbol …)`, an option without a value,
/// and an option stated twice.
///
/// `label` says whether `at` names a position only (a space's layer, say), so an error found
/// after the head is read names the form too — `layer 2 (fallback)` — or already names the
/// form (`door 1`).
fn parts<'a>(form: &'a Sexpr, at: &str, label: bool) -> Result<Parts<'a>> {
    let Sexpr::List(items) = form else {
        return Err(ArrangementError::new(
            at,
            format!("expected a form `( … )`, found {}", show(form)),
        ));
    };
    let Some((first, rest)) = items.split_first() else {
        return Err(ArrangementError::new(at, "an empty form `()`"));
    };
    let Sexpr::Symbol(head) = first else {
        return Err(ArrangementError::new(
            at,
            format!(
                "a form starts with its kind, a symbol; found {}",
                show(first)
            ),
        ));
    };
    let labeled = format!("{at} ({head})");
    let at = if label { labeled.as_str() } else { at };
    let mut args = Vec::new();
    let mut options = BTreeMap::new();
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match keyword(item) {
            Some(key) => {
                let value = items
                    .next()
                    .filter(|v| keyword(v).is_none())
                    .ok_or_else(|| {
                        ArrangementError::new(at, format!("option `:{key}` has no value"))
                    })?;
                if options.insert(key, value).is_some() {
                    return Err(ArrangementError::new(
                        at,
                        format!("option `:{key}` is stated twice"),
                    ));
                }
            }
            None => args.push(item),
        }
    }
    Ok(Parts {
        head: head.as_str(),
        args,
        options,
    })
}

/// `Some("key")` for an option keyword `:key`.
fn keyword(item: &Sexpr) -> Option<&str> {
    match item {
        Sexpr::Symbol(s) if s.len() > 1 && s.starts_with(':') => Some(&s[1..]),
        _ => None,
    }
}

/// A short rendering of a form for a message.
fn show(form: &Sexpr) -> String {
    let text = crate::write(form);
    if text.chars().count() > 48 {
        format!("`{}…`", text.chars().take(47).collect::<String>())
    } else {
        format!("`{text}`")
    }
}

/// A string part, or the refusal naming what it should have been.
fn string<'a>(value: &'a Sexpr, at: &str, what: &str) -> Result<&'a str> {
    match value {
        Sexpr::Str(s) => Ok(s),
        other => Err(ArrangementError::new(
            at,
            format!("{what} is a string, found {}", show(other)),
        )),
    }
}

/// A node's claimed identity: an IRI, never one under the skolem prefix.
fn identity(value: &Sexpr, at: &str, what: &str) -> Result<Iri> {
    let text = string(value, at, what)?;
    if text.starts_with(SKOLEM_PREFIX) {
        return Err(ArrangementError::new(
            at,
            format!(
                "`{text}` is under `{SKOLEM_PREFIX}`, where anonymous spaces are numbered, so it \
                 would read back as anonymous: an anonymous space is written without `:id`"
            ),
        ));
    }
    Iri::parse(text).map_err(|e| ArrangementError::new(at, format!("`{text}` is not an IRI: {e}")))
}

/// A `:match` value.
fn match_kind(value: &Sexpr, at: &str) -> Result<MatchKind> {
    let Sexpr::Symbol(word) = value else {
        return Err(ArrangementError::new(
            at,
            format!(
                "`:match` is one of exact, template, prefix — a symbol; found {}",
                show(value)
            ),
        ));
    };
    match MatchKind::from_keyword(word) {
        Some(MatchKind::Custom) => Err(ArrangementError::new(
            at,
            "`:match custom` is a grammar written outside core: it is described by its pattern and \
             not defined by it, so a declaration cannot rebuild it from the text",
        )),
        Some(kind) => Ok(kind),
        None => Err(ArrangementError::new(
            at,
            format!("`:match {word}` is not a kind of match (exact, template, prefix)"),
        )),
    }
}

/// The kind a door's pattern reads as: a template when it has a `{`, else an exact name.
fn door_inference(pattern: &str) -> MatchKind {
    if pattern.contains('{') {
        MatchKind::Template
    } else {
        MatchKind::Exact
    }
}

/// The kind a limiter's family reads as: a template when it has a `{`, else a prefix.
fn limit_inference(family: &str) -> MatchKind {
    if family.contains('{') {
        MatchKind::Template
    } else {
        MatchKind::Prefix
    }
}

/// Refuse a template pattern that does not parse.
fn check_template(pattern: &str, at: &str) -> Result<()> {
    UriTemplate::parse(pattern)
        .map(|_| ())
        .map_err(|e| ArrangementError::new(at, format!("`{pattern}` is not a URI template: {e}")))
}

/// The refusal for a form that is a kind core refuses to build, or no kind at all.
fn not_a_space(head: &str, at: &str) -> ArrangementError {
    let reason = match head {
        "opaque" => "an opaque space — a remote peer or a hand-written resolver — has nothing to \
                     rebuild from: a declaration arranges local endpoints only (declared remote \
                     mounts are ledger #630)"
            .to_string(),
        "rewrite" => "a closure rewrite's rule is code, not a table, so it cannot be declared: a \
                      table-driven rewrite is an `(alias …)`"
            .to_string(),
        "chain" => "a resolution chain is the arrangement seen from one request, not a space: \
                    declare the root layer"
            .to_string(),
        "confine" => "a confinement lives at a door, not where a space belongs: declare it as \
                      that door's `:confined` corridor"
            .to_string(),
        "door" => "a `(door …)` belongs inside `(endpoints …)`, not where a space goes".to_string(),
        "exact" | "prefix" => format!(
            "an `({head} …)` rule belongs inside `(alias …)`, ahead of the space it encloses"
        ),
        other => format!(
            "unknown form `({other} …)`: a space is one of endpoints, fallback, mount, alias, \
             limit, level, ref"
        ),
    };
    ArrangementError::new(at, reason)
}

impl Reader {
    /// Meet a node as core would (see [`Tally::meet`]), refusing at `here` with core's error.
    fn meet(
        &mut self,
        id: Option<&Iri>,
        kind: &SpaceKind,
        depth: usize,
        again: Option<&str>,
        here: &str,
    ) -> Result<Option<String>> {
        self.tally
            .meet(id, kind, depth, again)
            .map_err(|e| ArrangementError::core(here, e))
    }

    /// Read one space form at `depth` (the root is 1), `again` naming the space placed again
    /// that it lies under, if any. Each form reads its own parts first, then is met — named,
    /// its depth checked, its share charged — and only then are the spaces it encloses read,
    /// so the bounds stop a hostile document before the recursion or the expansion does.
    fn space(
        &mut self,
        form: &Sexpr,
        at: &str,
        depth: usize,
        again: Option<&str>,
    ) -> Result<Topology> {
        let bare = |head: &str| format!("{at} ({head})");
        let p = parts(form, at, true)?;
        let head = p.head;
        let id = match p.option("id") {
            Some(value) if head != "level" && head != "ref" => {
                Some(identity(value, &bare(head), "`:id`")?)
            }
            _ => None,
        };
        let here = match &id {
            Some(id) => format!("{at} ({head} <{}>)", id.as_str()),
            None => bare(head),
        };
        let tree = match head {
            "endpoints" => {
                p.allow(&here, &["id"])?;
                let mut doors = Vec::with_capacity(p.args.len());
                let mut corridors = Vec::new();
                for (i, arg) in p.args.iter().enumerate() {
                    let at = format!("{here} › door {}", i + 1);
                    let (door, corridor) = Self::door(arg, &at)?;
                    if let Some(corridor) = corridor {
                        corridors.push((i, corridor, at));
                    }
                    doors.push(door);
                }
                let mut kind = SpaceKind::EndpointSpace { doors };
                let below = self.meet(id.as_ref(), &kind, depth, again, &here)?;
                if let SpaceKind::EndpointSpace { doors } = &mut kind {
                    for (i, form, at) in corridors {
                        let at = format!("{at} › :confined");
                        let corridor = self.space(form, &at, depth + 1, below.as_deref())?;
                        if corridor.id.is_none() {
                            return Err(ArrangementError::new(
                                at,
                                "a confined corridor is named by its confinement: give it an \
                                 `:id`",
                            ));
                        }
                        doors[i].confined = Some(Box::new(corridor));
                    }
                }
                Topology::new(kind).with_id(id)
            }
            "fallback" => {
                p.allow(&here, &["id"])?;
                let kind = SpaceKind::Fallback;
                let below = self.meet(id.as_ref(), &kind, depth, again, &here)?;
                let mut tree = Topology::new(kind).with_id(id);
                for (i, arg) in p.args.iter().enumerate() {
                    tree = tree.child(self.space(
                        arg,
                        &format!("{here} › layer {}", i + 1),
                        depth + 1,
                        below.as_deref(),
                    )?);
                }
                tree
            }
            "mount" => {
                p.allow(&here, &["id"])?;
                let [prefix, inner] = p.args[..] else {
                    return Err(count(
                        &here,
                        "(mount …) takes a prefix and one space",
                        &p.args,
                    ));
                };
                let prefix = string(prefix, &here, "a mount's prefix")?;
                let kind = SpaceKind::Mount {
                    prefix: prefix.to_string(),
                };
                let below = self.meet(id.as_ref(), &kind, depth, again, &here)?;
                let inner = self.space(
                    inner,
                    &format!("{here} › space"),
                    depth + 1,
                    below.as_deref(),
                )?;
                Topology::new(kind).with_id(id).child(inner)
            }
            "alias" => {
                p.allow(&here, &["id", "max-hops"])?;
                let max_hops = match p.option("max-hops") {
                    None => DEFAULT_MAX_HOPS,
                    Some(Sexpr::Int(n)) if *n >= 1 => usize::try_from(*n).map_err(|_| {
                        ArrangementError::new(&here, format!("`:max-hops {n}` is too large"))
                    })?,
                    Some(other) => {
                        return Err(ArrangementError::new(
                            &here,
                            format!(
                                "`:max-hops` is a positive count (an alias follows at least one \
                                 hop), found {}",
                                show(other)
                            ),
                        ))
                    }
                };
                let Some((inner, rules)) = p.args.split_last() else {
                    return Err(ArrangementError::new(
                        &here,
                        "(alias …) takes its rules and then the one space it encloses: missing \
                         the space",
                    ));
                };
                if let Sexpr::List(items) = inner {
                    if let Some(Sexpr::Symbol(h)) = items.first() {
                        if h == "exact" || h == "prefix" {
                            return Err(ArrangementError::new(
                                &here,
                                "(alias …) ends with the one space it encloses, after its rules: \
                                 missing the space",
                            ));
                        }
                    }
                }
                let mut table = Vec::with_capacity(rules.len());
                for (i, rule) in rules.iter().enumerate() {
                    table.push(Self::rule(rule, &format!("{here} › rule {}", i + 1))?);
                }
                in_table_order(&mut table);
                let kind = SpaceKind::Alias {
                    rules: table,
                    max_hops,
                };
                let below = self.meet(id.as_ref(), &kind, depth, again, &here)?;
                let inner = self.space(
                    inner,
                    &format!("{here} › space"),
                    depth + 1,
                    below.as_deref(),
                )?;
                Topology::new(kind).with_id(id).child(inner)
            }
            "limit" => {
                p.allow(&here, &["id", "match"])?;
                let [family] = p.args[..] else {
                    return Err(count(&here, "(limit …) takes one family", &p.args));
                };
                let family = string(family, &here, "a limiter's family")?;
                let kind = match p.option("match") {
                    Some(value) => match_kind(value, &here)?,
                    None => limit_inference(family),
                };
                if kind == MatchKind::Template {
                    check_template(family, &here)?;
                }
                let kind = SpaceKind::Limit {
                    family: family.to_string(),
                    kind,
                };
                self.meet(id.as_ref(), &kind, depth, again, &here)?;
                Topology::new(kind).with_id(id)
            }
            "level" => {
                p.allow(&here, &["seals", "namespace"])?;
                let [name, inner] = p.args[..] else {
                    return Err(count(
                        &here,
                        "(level …) takes its name and one space",
                        &p.args,
                    ));
                };
                let name = identity(name, &here, "a level's name")?;
                let here = format!("{at} (level <{}>)", name.as_str());
                let mut seals = Vec::new();
                if let Some(value) = p.option("seals") {
                    let Sexpr::List(items) = value else {
                        return Err(ArrangementError::new(
                            &here,
                            format!("`:seals` is a list of prefixes, found {}", show(value)),
                        ));
                    };
                    for item in items {
                        let prefix = string(item, &here, "a sealed prefix")?.to_string();
                        if seals.contains(&prefix) {
                            return Err(ArrangementError::new(
                                &here,
                                format!(
                                    "`{prefix}` is sealed twice: the Turtle states a seal once, so \
                                     the second would not survive the round trip"
                                ),
                            ));
                        }
                        seals.push(prefix);
                    }
                }
                let namespace = match p.option("namespace") {
                    Some(value) => Some(string(value, &here, "`:namespace`")?.to_string()),
                    None => None,
                };
                let kind = SpaceKind::Level { seals, namespace };
                let below = self.meet(Some(&name), &kind, depth, again, &here)?;
                let inner = self.space(
                    inner,
                    &format!("{here} › space"),
                    depth + 1,
                    below.as_deref(),
                )?;
                Topology::new(kind).with_id(Some(name)).child(inner)
            }
            "ref" => {
                p.allow(&here, &[])?;
                let [name] = p.args[..] else {
                    return Err(count(
                        &here,
                        "(ref …) takes the IRI of a named space",
                        &p.args,
                    ));
                };
                let name = string(name, &here, "a ref's target")?;
                let here = format!("{at} (ref <{name}>)");
                let Some(claimed) = self.named.get(name) else {
                    return Err(ArrangementError::new(
                        &here,
                        format!(
                            "no space named <{name}> is declared before this point: a ref names \
                             a space written out in full earlier in the document"
                        ),
                    ));
                };
                // A space placed again: core charges everything under it to its name, or to
                // the space placed again that this ref itself lies under.
                let me = again.unwrap_or(name);
                self.tally
                    .again(claimed, depth, me)
                    .map_err(|e| ArrangementError::core(&here, e))?;
                return Ok(claimed.tree.clone());
            }
            other => return Err(not_a_space(other, &bare(other))),
        };
        self.claim(tree, &here)
    }

    /// Record a named space, or check a restatement against the first.
    fn claim(&mut self, tree: Topology, here: &str) -> Result<Topology> {
        let Some(id) = tree.id.as_ref().map(|id| id.as_str().to_string()) else {
            return Ok(tree);
        };
        match self.named.get(&id) {
            Some(first) if first.tree == tree => Ok(tree),
            Some(_) => Err(ArrangementError::new(
                here,
                format!(
                    "two different arrangements claim <{id}>: a name is a claim — same name, same \
                     doors — so write `(ref \"{id}\")` to place the same space again"
                ),
            )),
            None => {
                let (nodes, text, height) = size(&tree);
                self.named.insert(
                    id,
                    Claimed {
                        tree: tree.clone(),
                        nodes,
                        text,
                        height,
                    },
                );
                Ok(tree)
            }
        }
    }

    /// Read one `(door …)`'s own parts, returning the door and its `:confined` corridor's
    /// form, unread: the corridor is a space of its own, read after the space that holds the
    /// door has been met, as core meets them.
    fn door<'a>(form: &'a Sexpr, at: &str) -> Result<(Door, Option<&'a Sexpr>)> {
        let p = parts(form, at, false)?;
        if p.head != "door" {
            return Err(ArrangementError::new(
                at,
                format!(
                    "(endpoints …) holds only `(door \"pattern\" endpoint)` forms; found `({} …)`",
                    p.head
                ),
            ));
        }
        p.allow(at, &["match", "confined"])?;
        let [pattern, endpoint] = p.args[..] else {
            return Err(count(
                at,
                "(door …) takes a pattern and the name of the endpoint it binds",
                &p.args,
            ));
        };
        let pattern = string(pattern, at, "a door's pattern")?;
        let endpoint = match endpoint {
            Sexpr::Symbol(name) => name.as_str(),
            Sexpr::Str(name) => name.as_str(),
            other => {
                return Err(ArrangementError::new(
                    at,
                    format!(
                        "an endpoint name is a symbol, or a string when it is not a plain \
                         symbol; found {}",
                        show(other)
                    ),
                ))
            }
        };
        let kind = match p.option("match") {
            Some(value) => match_kind(value, at)?,
            None => door_inference(pattern),
        };
        match kind {
            MatchKind::Prefix => {
                return Err(ArrangementError::new(
                    at,
                    format!(
                        "`{pattern}` is declared a prefix: a door is an exact name or a template, \
                         and a prefix is a mount's or a limiter's"
                    ),
                ))
            }
            MatchKind::Template => check_template(pattern, at)?,
            _ => {}
        }
        Ok((Door::new(pattern, kind, endpoint), p.option("confined")))
    }

    /// Read one alias rule.
    fn rule(form: &Sexpr, at: &str) -> Result<TopologyRule> {
        let p = parts(form, at, false)?;
        let kind = match p.head {
            "exact" => RuleKind::Exact,
            "prefix" => RuleKind::Prefix,
            other => {
                return Err(ArrangementError::new(
                    at,
                    format!(
                        "an alias's rules are `(exact \"from\" \"to\")` or `(prefix \"from\" \
                         \"to\")`, ahead of the one space it encloses; found `({other} …)`"
                    ),
                ))
            }
        };
        p.allow(at, &[])?;
        let [from, to] = p.args[..] else {
            return Err(count(
                at,
                &format!(
                    "({} …) takes the name it matches and the name it rewrites to",
                    p.head
                ),
                &p.args,
            ));
        };
        Ok(TopologyRule::new(
            kind,
            string(from, at, "a rule's name")?,
            string(to, at, "a rule's rewrite")?,
        ))
    }
}

/// Put alias rules in the order core's [`AliasTable`](ikigai_core::AliasTable) holds them:
/// most specific first — the longest name first, an exact rule ahead of a prefix one at equal
/// length, then by name — and, the sort being stable, in the order written where two rules tie.
/// A table is consulted in that order whatever order its rules were added in, so the order an
/// author writes them in says nothing, and reading them in the table's order is what makes a
/// declared alias the same arrangement as the coded one. `tests/arrangement.rs` checks this
/// against a real `AliasTable`, so a change to core's order fails here rather than drifting.
fn in_table_order(rules: &mut [TopologyRule]) {
    rules.sort_by(|a, b| {
        b.from
            .len()
            .cmp(&a.from.len())
            .then_with(|| a.kind.keyword().cmp(b.kind.keyword()))
            .then_with(|| a.from.cmp(&b.from))
    });
}

/// `tree` with every alias's rules in table order: what reading it back yields.
fn in_table_order_throughout(tree: &Topology) -> Topology {
    let kind = match &tree.kind {
        SpaceKind::Alias { rules, max_hops } => {
            let mut rules = rules.clone();
            in_table_order(&mut rules);
            SpaceKind::Alias {
                rules,
                max_hops: *max_hops,
            }
        }
        SpaceKind::EndpointSpace { doors } => SpaceKind::EndpointSpace {
            doors: doors
                .iter()
                .map(|door| {
                    let mut out = Door::new(door.pattern.clone(), door.kind, door.endpoint.clone());
                    if let Some(corridor) = &door.confined {
                        out = out.confined_to(in_table_order_throughout(corridor));
                    }
                    out
                })
                .collect(),
        },
        other => other.clone(),
    };
    tree.children.iter().fold(
        Topology::new(kind).with_id(tree.id.clone()),
        |node, child| node.child(in_table_order_throughout(child)),
    )
}

/// The refusal for a form with the wrong number of parts.
fn count(at: &str, takes: &str, args: &[&Sexpr]) -> ArrangementError {
    ArrangementError::new(
        at,
        format!(
            "{takes}; found {} part{}",
            args.len(),
            if args.len() == 1 { "" } else { "s" }
        ),
    )
}

// =====================================================================================
// Topology → s-expression (the canonical printer)
// =====================================================================================

/// **Print an arrangement** in canonical form: one space per line, indented two spaces per
/// level, options after the positional parts and only where they say something the pattern
/// does not (`:match` against the inference, `:max-hops` against the default), and a named
/// space written in full where it is first met — in pre-order, as core's Turtle writes it —
/// and as `(ref "iri")` after that.
///
/// Refuses what the grammar cannot say, which is exactly what core cannot build: an opaque
/// space, a closure rewrite, a chain, a confinement where a space belongs, a door that
/// matches by prefix or by a custom grammar, a template that does not parse, and two different
/// arrangements under one name (core's own Turtle would keep only the first).
///
/// ```
/// use ikigai_core::{Door, MatchKind, SpaceKind, Topology};
/// use ikigai_sexpr::topology_to_arrangement;
///
/// let tree = Topology::new(SpaceKind::Fallback)
///     .child(Topology::new(SpaceKind::Limit {
///         family: "urn:personal:".into(),
///         kind: MatchKind::Prefix,
///     }))
///     .child(Topology::new(SpaceKind::EndpointSpace {
///         doors: vec![Door::new("urn:public:hello", MatchKind::Exact, "hello")],
///     }));
/// assert_eq!(
///     topology_to_arrangement(&tree).unwrap(),
///     "(fallback\n  (limit \"urn:personal:\")\n  (endpoints\n    (door \"urn:public:hello\" hello)))\n"
/// );
/// ```
pub fn topology_to_arrangement(tree: &Topology) -> Result<String> {
    check(tree)?;
    let block = Printer::default().space(tree, "root")?;
    let mut out = String::new();
    block.render(0, &mut out);
    out.push('\n');
    Ok(out)
}

/// One printed form: its header (head, positional parts, options) and the forms it
/// encloses, each on its own line.
struct Block {
    header: String,
    children: Vec<Block>,
}

impl Block {
    fn leaf(header: String) -> Block {
        Block {
            header,
            children: Vec::new(),
        }
    }

    fn render(&self, indent: usize, out: &mut String) {
        out.push_str(&" ".repeat(indent));
        out.push('(');
        out.push_str(&self.header);
        for child in &self.children {
            out.push('\n');
            child.render(indent + 2, out);
        }
        out.push(')');
    }
}

#[derive(Default)]
struct Printer {
    /// Every named node printed so far, as it was first met.
    seen: BTreeMap<String, Topology>,
}

/// An endpoint name as the reader will read it back: a bare symbol when it is one, else a
/// string.
fn endpoint_token(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(':')
        && !name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '"' | ';'))
        && name.parse::<i64>().is_err();
    if plain {
        name.to_string()
    } else {
        render_string_literal(name)
    }
}

impl Printer {
    fn space(&mut self, tree: &Topology, at: &str) -> Result<Block> {
        let head = match &tree.kind {
            SpaceKind::EndpointSpace { .. } => "endpoints",
            SpaceKind::Fallback => "fallback",
            SpaceKind::Mount { .. } => "mount",
            SpaceKind::Alias { .. } => "alias",
            SpaceKind::Limit { .. } => "limit",
            SpaceKind::Level { .. } => "level",
            SpaceKind::Opaque => "opaque",
            SpaceKind::Rewrite => "rewrite",
            SpaceKind::Chain { .. } => "chain",
            SpaceKind::Confine => "confine",
            _ => "unknown",
        };
        let here = match &tree.id {
            Some(id) => format!("{at} ({head} <{}>)", id.as_str()),
            None => format!("{at} ({head})"),
        };
        if let Some(id) = &tree.id {
            let id = id.as_str();
            if id.starts_with(SKOLEM_PREFIX) {
                return Err(ArrangementError::new(
                    &here,
                    format!(
                        "<{id}> is under `{SKOLEM_PREFIX}`, where anonymous spaces are numbered"
                    ),
                ));
            }
            match self.seen.get(id) {
                Some(first) if first == tree => {
                    return Ok(Block::leaf(format!("ref {}", render_string_literal(id))))
                }
                Some(_) => {
                    return Err(ArrangementError::new(
                        &here,
                        format!(
                            "two different arrangements claim <{id}>: a name is a claim — same \
                             name, same doors — and core's Turtle would keep only the first"
                        ),
                    ))
                }
                None => {
                    self.seen.insert(id.to_string(), tree.clone());
                }
            }
        }
        let expect = |n: usize| {
            if tree.children.len() == n {
                Ok(())
            } else {
                Err(ArrangementError::new(
                    &here,
                    format!(
                        "encloses {} spaces, and a `{head}` encloses {n}",
                        tree.children.len()
                    ),
                ))
            }
        };
        let id_option = |header: &mut String| {
            if let Some(id) = &tree.id {
                header.push_str(" :id ");
                header.push_str(&render_string_literal(id.as_str()));
            }
        };
        let mut header = head.to_string();
        let mut children = Vec::new();
        match &tree.kind {
            SpaceKind::EndpointSpace { doors } => {
                expect(0)?;
                id_option(&mut header);
                for (i, door) in doors.iter().enumerate() {
                    children.push(self.door(door, &format!("{here} › door {}", i + 1))?);
                }
            }
            SpaceKind::Fallback => {
                id_option(&mut header);
                for (i, child) in tree.children.iter().enumerate() {
                    children.push(self.space(child, &format!("{here} › layer {}", i + 1))?);
                }
            }
            SpaceKind::Mount { prefix } => {
                expect(1)?;
                header.push(' ');
                header.push_str(&render_string_literal(prefix));
                id_option(&mut header);
                children.push(self.space(&tree.children[0], &format!("{here} › space"))?);
            }
            SpaceKind::Alias { rules, max_hops } => {
                expect(1)?;
                if *max_hops == 0 {
                    return Err(ArrangementError::new(
                        &here,
                        "an alias follows at least one hop (`:max-hops` ≥ 1)",
                    ));
                }
                id_option(&mut header);
                if *max_hops != DEFAULT_MAX_HOPS {
                    header.push_str(&format!(" :max-hops {max_hops}"));
                }
                let mut rules = rules.clone();
                in_table_order(&mut rules);
                for rule in &rules {
                    children.push(Block::leaf(format!(
                        "{} {} {}",
                        rule.kind.keyword(),
                        render_string_literal(&rule.from),
                        render_string_literal(&rule.to)
                    )));
                }
                children.push(self.space(&tree.children[0], &format!("{here} › space"))?);
            }
            SpaceKind::Limit { family, kind } => {
                expect(0)?;
                header.push(' ');
                header.push_str(&render_string_literal(family));
                id_option(&mut header);
                match kind {
                    MatchKind::Custom => {
                        return Err(ArrangementError::new(
                            &here,
                            format!(
                                "`{family}` is a custom grammar's description, not its definition: \
                                 the grammar is code, and a declaration cannot rebuild it"
                            ),
                        ))
                    }
                    MatchKind::Template => check_template(family, &here)?,
                    _ => {}
                }
                if *kind != limit_inference(family) {
                    header.push_str(" :match ");
                    header.push_str(kind.keyword());
                }
            }
            SpaceKind::Level { seals, namespace } => {
                expect(1)?;
                let Some(name) = &tree.id else {
                    return Err(ArrangementError::new(&here, "a level is always named"));
                };
                header = format!("level {}", render_string_literal(name.as_str()));
                if !seals.is_empty() {
                    let mut unique = BTreeSet::new();
                    for seal in seals {
                        if !unique.insert(seal) {
                            return Err(ArrangementError::new(
                                &here,
                                format!("`{seal}` is sealed twice: the Turtle states a seal once"),
                            ));
                        }
                    }
                    let list: Vec<String> =
                        seals.iter().map(|s| render_string_literal(s)).collect();
                    header.push_str(&format!(" :seals ({})", list.join(" ")));
                }
                if let Some(namespace) = namespace {
                    header.push_str(" :namespace ");
                    header.push_str(&render_string_literal(namespace));
                }
                children.push(self.space(&tree.children[0], &format!("{here} › space"))?);
            }
            SpaceKind::Opaque
            | SpaceKind::Rewrite
            | SpaceKind::Chain { .. }
            | SpaceKind::Confine => return Err(not_a_space(head, &here)),
            _ => {
                return Err(ArrangementError::new(
                    &here,
                    "a kind of space this crate does not know, so it cannot say it",
                ))
            }
        }
        Ok(Block { header, children })
    }

    fn door(&mut self, door: &Door, at: &str) -> Result<Block> {
        let pattern = &door.pattern;
        match door.kind {
            MatchKind::Exact => {}
            MatchKind::Template => check_template(pattern, at)?,
            MatchKind::Prefix => {
                return Err(ArrangementError::new(
                    at,
                    format!(
                        "`{pattern}` is a prefix door: a door is an exact name or a template, and \
                         a prefix is a mount's or a limiter's"
                    ),
                ))
            }
            _ => {
                return Err(ArrangementError::new(
                    at,
                    format!(
                        "`{pattern}` is a custom grammar's description, not its definition: the \
                         grammar is code, and a declaration cannot rebuild it"
                    ),
                ))
            }
        }
        let mut header = format!(
            "door {} {}",
            render_string_literal(pattern),
            endpoint_token(&door.endpoint)
        );
        if door.kind != door_inference(pattern) {
            header.push_str(" :match ");
            header.push_str(door.kind.keyword());
        }
        let mut children = Vec::new();
        if let Some(corridor) = &door.confined {
            if corridor.id.is_none() {
                return Err(ArrangementError::new(
                    at,
                    "a confined corridor is named by its confinement, and this one is anonymous",
                ));
            }
            header.push_str(" :confined");
            children.push(self.space(corridor, &format!("{at} › :confined"))?);
        }
        Ok(Block { header, children })
    }
}

// =====================================================================================
// s-expression ↔ Turtle, through core
// =====================================================================================

/// **An arrangement as Turtle**: read the s-expression ([`arrangement_to_topology`]), render
/// it with core's [`Topology::to_turtle`], and read the Turtle back with
/// [`Topology::from_turtle`] — refusing to answer unless it reads back as the same
/// arrangement. The Turtle is exactly what `urn:kernel:topology` would render for the same
/// tree, so a host builds it with [`build`](ikigai_core::build) as it would its own.
pub fn arrangement_to_turtle(src: &str) -> Result<String> {
    let tree = arrangement_to_topology(src)?;
    let turtle = tree.to_turtle();
    match Topology::from_turtle(&turtle) {
        Ok(back) if back == tree => Ok(turtle),
        Ok(_) => Err(ArrangementError::document(
            "the arrangement does not survive its own Turtle: core reads the rendering back as a \
             different arrangement (a node's `:id` colliding with an IRI core writes for a door, \
             a list cell or a rule?) — refused rather than transrepted lossily",
        )),
        Err(e) => {
            let mut refused = from_declaration(e);
            refused.reason = format!(
                "the arrangement does not survive its own Turtle: core refuses its rendering: {}",
                refused.reason
            );
            Err(refused)
        }
    }
}

/// **Turtle as an arrangement**: read it with core's [`Topology::from_turtle`] — which holds
/// it to core's declaration bounds as it reads, so a document that would expand past them is
/// refused with [`DeclarationError::TooLarge`] in [`ArrangementError::declaration`] — print it
/// in canonical form ([`topology_to_arrangement`]), and read the text back, refusing to answer
/// unless it reads back as the same arrangement.
pub fn turtle_to_arrangement(turtle: &str) -> Result<String> {
    let tree = Topology::from_turtle(turtle).map_err(from_declaration)?;
    let text = topology_to_arrangement(&tree)?;
    if arrangement_to_topology(&text)? != in_table_order_throughout(&tree) {
        return Err(ArrangementError::document(
            "the arrangement does not survive its own s-expression: the printed text reads back \
             as a different arrangement — refused rather than transrepted lossily",
        ));
    }
    Ok(text)
}

/// Core's refusal of a Turtle document, named as core names it, and kept typed in
/// [`ArrangementError::declaration`].
fn from_declaration(error: impl Into<Box<DeclarationError>>) -> ArrangementError {
    let error = error.into();
    match error.as_ref() {
        DeclarationError::Malformed {
            node: Some(node),
            reason,
        } => ArrangementError {
            at: format!("<{node}>"),
            reason: format!("not a declaration: {reason}"),
            declaration: Some(error.clone()),
        },
        DeclarationError::TooLarge { node, .. } => {
            ArrangementError::core(format!("<{node}>"), error.clone())
        }
        _ => ArrangementError::core("", error),
    }
}

// =====================================================================================
// The endpoints
// =====================================================================================

/// A refused arrangement is the ARGUMENT's problem: a typed `InvalidArgument` naming the input
/// the caller passed, prefixed with the stage that refused it.
fn invalid(arg: &str, iri: &str, err: &ArrangementError) -> CoreError {
    CoreError::InvalidArgument {
        name: arg.to_string(),
        detail: format!("{iri}: {err}"),
    }
}

/// `urn:sexpr:arrangement-to-rdf`: an arrangement written as an s-expression → the Turtle core
/// reads. A lossless `ik:Transreptor` (`text/x-ikigai-arrangement` → `text/turtle`), and a pure
/// function of its input, so `.cacheable()`.
pub(crate) struct ArrangementToRdf;

#[async_trait]
impl Endpoint for ArrangementToRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        const IRI: &str = "urn:sexpr:arrangement-to-rdf";
        let (src, arg) = read_source(inv)?;
        let turtle = arrangement_to_turtle(src).map_err(|e| invalid(arg, IRI, &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_TURTLE).with_param("charset", "utf-8"),
            turtle.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-arrangement-to-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-arrangement-to-rdf")
            .title("Arrangement (s-expression) to Turtle")
            .summary(
                "Transrept a kernel arrangement written as an s-expression (an *.arrangement \
                 file) to the ik: Turtle core builds a space from — LOSSLESSLY, checked by \
                 reading the Turtle back. Pipe the arrangement in (or pass `in=`): one space form, \
                 `(endpoints (door \"pattern\" endpoint)…)`, `(fallback space…)`, \
                 `(mount \"prefix\" space)`, `(alias (exact \"from\" \"to\")… space)`, \
                 `(limit \"family\")`, `(level \"iri\" :seals (\"prefix\"…) space)` or \
                 `(ref \"iri\")`, with `:id \"iri\"` naming a space. What core cannot build \
                 (opaque, rewrite, chain) is refused, never skipped. Comments are not kept.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the arrangement s-expression TEXT"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the arrangement s-expression TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_TURTLE)
            // Lossless (the default for a transreptor): the arrangement reads back identical.
            .transreptor([MEDIA_ARRANGEMENT], [MEDIA_TURTLE])
    }
}

/// `urn:sexpr:arrangement-from-rdf`: an arrangement's Turtle → its canonical s-expression. The
/// inverse of [`ArrangementToRdf`]; a lossless `ik:Transreptor` (`text/turtle` →
/// `text/x-ikigai-arrangement`), pure, so `.cacheable()`.
pub(crate) struct ArrangementFromRdf;

#[async_trait]
impl Endpoint for ArrangementFromRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        const IRI: &str = "urn:sexpr:arrangement-from-rdf";
        let (src, arg) = read_source(inv)?;
        let text = turtle_to_arrangement(src).map_err(|e| invalid(arg, IRI, &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_ARRANGEMENT).with_param("charset", "utf-8"),
            text.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-arrangement-from-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-arrangement-from-rdf")
            .title("Arrangement Turtle to s-expression")
            .summary(
                "Print a kernel arrangement's ik: Turtle (what urn:kernel:topology renders, and \
                 what core builds a space from) as its canonical s-expression — the inverse of \
                 urn:sexpr:arrangement-to-rdf, LOSSLESS and checked by reading the text back. \
                 Pipe the Turtle in (or pass `in=`). A named space met twice prints once, then as \
                 `(ref \"iri\")`. What the s-expression cannot say, because core cannot build it \
                 (an opaque space, a closure rewrite, a chain), is refused, never skipped.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the arrangement's Turtle TEXT"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the arrangement's Turtle TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_ARRANGEMENT)
            .transreptor([MEDIA_TURTLE], [MEDIA_ARRANGEMENT])
    }
}
