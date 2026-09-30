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
//! ## Bounds
//!
//! The text goes through the crate's bounded [`parse`] reader, and the tree it describes is
//! bounded again after `ref`s are expanded: at most [`MAX_ARRANGEMENT_DEPTH`] spaces and doors
//! deep and [`MAX_ARRANGEMENT_NODES`] in all. A Turtle document is measured against the same
//! bounds BEFORE core parses it, because core's reader expands a named node at every place
//! it is referenced.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, DeclarationError, Description, Door, Endpoint, Error as CoreError, Invocation, Iri,
    MatchKind, ReprType, Representation, Result as CoreResult, RuleKind, SpaceKind, Topology,
    TopologyRule, UriTemplate, Verb, DEFAULT_MAX_HOPS,
};
use oxrdf::{NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};

use crate::{
    content_summary, in_summary, parse, read_source, render_string_literal, Sexpr, MEDIA_TURTLE,
    XSD_STRING,
};

/// The media type of an arrangement written as an s-expression: an `*.arrangement` file.
/// DISTINCT from [`text/x-sexpr`](crate::MEDIA_SEXPR) on purpose: `urn:sexpr:to-rdf` and
/// `urn:rdf:from-sexpr` already transrept `text/x-sexpr` to Turtle, so a lossless selector over
/// that type could pick one of them and hand the builder a list graph or a domain graph.
pub const MEDIA_ARRANGEMENT: &str = "text/x-ikigai-arrangement";

/// How deep an arrangement may nest, counting every space and every door on the way down
/// (the root is 1). Real arrangements are a handful deep; the bound exists so a hostile
/// document is a clean refusal rather than a stack overflow in the recursive walks — this
/// crate's and core's (`to_turtle`, `from_turtle`, `build`). Set by measurement: core's
/// `from_turtle` is the heaviest, at roughly 19 KB of stack per level in a debug build (a
/// chain of fallbacks overflowed a 2 MiB thread at about 110 levels), so 48 fits a 1 MiB
/// thread and leaves a 2 MiB worker (tokio's default) twice the room it needs.
pub const MAX_ARRANGEMENT_DEPTH: usize = 48;

/// How many spaces and doors an arrangement may hold once every `ref` is expanded. A `ref`
/// is one line of text and a whole subtree of the tree, so without this a few lines that
/// reference each other double at every step (the "billion laughs" shape).
pub const MAX_ARRANGEMENT_NODES: usize = 65_536;

/// Where core numbers anonymous nodes: a node IRI under it reads back as anonymous, so an
/// `:id` there would not survive the round trip. Core keeps the constant crate-private; the
/// prefix is part of the Turtle it writes (`Topology::to_turtle`).
const SKOLEM_PREFIX: &str = "urn:ikigai:space:_:";

const IK: &str = "https://ikigai-rs.dev/ns#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The `ik:` classes a space node or a door is typed with: what the bounds count.
const COUNTED_KINDS: [&str; 11] = [
    "OpaqueSpace",
    "EndpointSpace",
    "Fallback",
    "Mount",
    "Rewrite",
    "Alias",
    "Limit",
    "Confine",
    "Level",
    "Chain",
    "Door",
];

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
}

impl ArrangementError {
    fn new(at: impl Into<String>, reason: impl Into<String>) -> Self {
        ArrangementError {
            at: at.into(),
            reason: reason.into(),
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
    Reader::default().space(&form, "root", 1)
}

/// A named space already read: the tree, and how much of the bounds it spends.
struct Claimed {
    tree: Topology,
    nodes: usize,
    height: usize,
}

#[derive(Default)]
struct Reader {
    /// Every named space read so far, by IRI.
    named: BTreeMap<String, Claimed>,
    /// Spaces and doors in the tree so far, with every `ref` expanded.
    nodes: usize,
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

/// How many spaces and doors a tree holds, and how deep it goes.
fn measure(tree: &Topology) -> (usize, usize) {
    let mut nodes = 1;
    let mut height = 0;
    if let SpaceKind::EndpointSpace { doors } = &tree.kind {
        for door in doors {
            nodes += 1;
            let mut below = 0;
            if let Some(corridor) = &door.confined {
                let (n, h) = measure(corridor);
                nodes += n;
                below = h;
            }
            height = height.max(1 + below);
        }
    }
    for child in &tree.children {
        let (n, h) = measure(child);
        nodes += n;
        height = height.max(h);
    }
    (nodes, height + 1)
}

impl Reader {
    /// Spend `n` nodes of the bound.
    fn spend(&mut self, n: usize, at: &str) -> Result<()> {
        self.nodes = self.nodes.saturating_add(n);
        if self.nodes > MAX_ARRANGEMENT_NODES {
            return Err(ArrangementError::new(
                at,
                format!(
                    "the arrangement holds more than {MAX_ARRANGEMENT_NODES} spaces and doors \
                     with every `ref` expanded"
                ),
            ));
        }
        Ok(())
    }

    fn deep(depth: usize, at: &str) -> Result<()> {
        if depth > MAX_ARRANGEMENT_DEPTH {
            return Err(ArrangementError::new(
                at,
                format!(
                    "the arrangement nests deeper than {MAX_ARRANGEMENT_DEPTH} spaces and doors"
                ),
            ));
        }
        Ok(())
    }

    /// Read one space form at `depth` (the root is 1).
    fn space(&mut self, form: &Sexpr, at: &str, depth: usize) -> Result<Topology> {
        Self::deep(depth, at)?;
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
        if head != "ref" {
            self.spend(1, &here)?;
        }
        let tree = match head {
            "endpoints" => {
                p.allow(&here, &["id"])?;
                let mut doors = Vec::with_capacity(p.args.len());
                for (i, arg) in p.args.iter().enumerate() {
                    doors.push(self.door(arg, &format!("{here} › door {}", i + 1), depth + 1)?);
                }
                Topology::new(SpaceKind::EndpointSpace { doors }).with_id(id)
            }
            "fallback" => {
                p.allow(&here, &["id"])?;
                let mut tree = Topology::new(SpaceKind::Fallback).with_id(id);
                for (i, arg) in p.args.iter().enumerate() {
                    tree = tree.child(self.space(
                        arg,
                        &format!("{here} › layer {}", i + 1),
                        depth + 1,
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
                Topology::new(SpaceKind::Mount {
                    prefix: prefix.to_string(),
                })
                .with_id(id)
                .child(self.space(
                    inner,
                    &format!("{here} › space"),
                    depth + 1,
                )?)
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
                Topology::new(SpaceKind::Alias {
                    rules: table,
                    max_hops,
                })
                .with_id(id)
                .child(self.space(
                    inner,
                    &format!("{here} › space"),
                    depth + 1,
                )?)
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
                Topology::new(SpaceKind::Limit {
                    family: family.to_string(),
                    kind,
                })
                .with_id(id)
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
                let inner = self.space(inner, &format!("{here} › space"), depth + 1)?;
                Topology::new(SpaceKind::Level { seals, namespace })
                    .with_id(Some(name))
                    .child(inner)
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
                let (tree, nodes, height) = (claimed.tree.clone(), claimed.nodes, claimed.height);
                Self::deep(depth - 1 + height, &here)?;
                self.spend(nodes, &here)?;
                return Ok(tree);
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
                let (nodes, height) = measure(&tree);
                self.named.insert(
                    id,
                    Claimed {
                        tree: tree.clone(),
                        nodes,
                        height,
                    },
                );
                Ok(tree)
            }
        }
    }

    /// Read one `(door …)` at `depth`.
    fn door(&mut self, form: &Sexpr, at: &str, depth: usize) -> Result<Door> {
        Self::deep(depth, at)?;
        self.spend(1, at)?;
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
        let mut door = Door::new(pattern, kind, endpoint);
        if let Some(corridor) = p.option("confined") {
            let corridor = self.space(corridor, &format!("{at} › :confined"), depth + 1)?;
            if corridor.id.is_none() {
                return Err(ArrangementError::new(
                    format!("{at} › :confined"),
                    "a confined corridor is named by its confinement: give it an `:id`",
                ));
            }
            door = door.confined_to(corridor);
        }
        Ok(door)
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
    let (nodes, height) = measure(tree);
    if height > MAX_ARRANGEMENT_DEPTH {
        return Err(ArrangementError::new(
            "root",
            format!("the arrangement nests deeper than {MAX_ARRANGEMENT_DEPTH} spaces and doors"),
        ));
    }
    if nodes > MAX_ARRANGEMENT_NODES {
        return Err(ArrangementError::new(
            "root",
            format!("the arrangement holds more than {MAX_ARRANGEMENT_NODES} spaces and doors"),
        ));
    }
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
        Err(e) => Err(ArrangementError::document(format!(
            "the arrangement does not survive its own Turtle: core refuses its rendering: {e}"
        ))),
    }
}

/// **Turtle as an arrangement**: measure the document against the bounds, read it with
/// core's [`Topology::from_turtle`], print it in canonical form
/// ([`topology_to_arrangement`]), and read the text back — refusing to answer unless it reads
/// back as the same arrangement.
pub fn turtle_to_arrangement(turtle: &str) -> Result<String> {
    measure_turtle(turtle)?;
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

/// Core's refusal of a Turtle document, named as core names it.
fn from_declaration(error: DeclarationError) -> ArrangementError {
    match error {
        DeclarationError::Malformed {
            node: Some(node),
            reason,
        } => ArrangementError::new(format!("<{node}>"), format!("not a declaration: {reason}")),
        other => ArrangementError::document(other.to_string()),
    }
}

/// Measure a Turtle document against [`MAX_ARRANGEMENT_DEPTH`] and [`MAX_ARRANGEMENT_NODES`]
/// BEFORE core reads it, in the same units the s-expression reader counts: every node typed
/// as a space or a door is one, reached through every path that reaches it (core's reader
/// expands a named node at each reference). List cells, rules and `rdf:nil` count nothing.
/// Iterative, so a hostile document cannot overflow the stack here either. A cycle is left
/// for core to refuse by name.
fn measure_turtle(turtle: &str) -> Result<()> {
    let mut stated = HashSet::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut counted: HashSet<String> = HashSet::new();
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_slice(turtle.as_bytes()) {
        let quad = quad.map_err(|e| ArrangementError::document(format!("not Turtle: {e}")))?;
        let NamedOrBlankNode::NamedNode(subject) = &quad.subject else {
            continue; // core refuses a blank node by name
        };
        let Term::NamedNode(object) = &quad.object else {
            continue;
        };
        let (s, o) = (subject.as_str(), object.as_str());
        if quad.predicate.as_str() == RDF_TYPE {
            if o.strip_prefix(IK)
                .is_some_and(|kind| COUNTED_KINDS.contains(&kind))
            {
                counted.insert(s.to_string());
            }
            continue;
        }
        if stated.insert((
            s.to_string(),
            quad.predicate.as_str().to_string(),
            o.to_string(),
        )) {
            edges.entry(s.to_string()).or_default().push(o.to_string());
        }
    }

    // (nodes, height) per node, with every path expanded.
    let mut memo: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut on_stack: HashSet<&str> = HashSet::new();
    let none: &[String] = &[];
    let roots: Vec<&str> = edges.keys().map(String::as_str).collect();
    for root in roots {
        if memo.contains_key(root) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(root, 0)];
        on_stack.insert(root);
        while let Some(&(node, next)) = stack.last() {
            let successors = edges.get(node).map_or(none, Vec::as_slice);
            if let Some(successor) = successors.get(next) {
                stack.last_mut().expect("the stack is not empty").1 += 1;
                let successor = successor.as_str();
                if !memo.contains_key(successor) && on_stack.insert(successor) {
                    stack.push((successor, 0));
                }
                continue;
            }
            let own = usize::from(counted.contains(node));
            let (mut nodes, mut height) = (own, 0);
            for successor in successors {
                if let Some(&(n, h)) = memo.get(successor.as_str()) {
                    nodes = nodes.saturating_add(n);
                    height = height.max(h);
                }
            }
            let height = height + own;
            if height > MAX_ARRANGEMENT_DEPTH {
                return Err(ArrangementError::new(
                    format!("<{node}>"),
                    format!("the arrangement nests deeper than {MAX_ARRANGEMENT_DEPTH} spaces and doors"),
                ));
            }
            if nodes > MAX_ARRANGEMENT_NODES {
                return Err(ArrangementError::new(
                    format!("<{node}>"),
                    format!(
                        "the arrangement holds more than {MAX_ARRANGEMENT_NODES} spaces and doors \
                         with every reference to a named space expanded"
                    ),
                ));
            }
            memo.insert(node, (nodes, height));
            on_stack.remove(node);
            stack.pop();
        }
    }
    Ok(())
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
