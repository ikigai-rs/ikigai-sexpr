//! **The lossless code-as-graph codec** — `&Sexpr` ⇄ RDF Turtle, kernel-free. Behind the
//! `full` feature: it is the one part of the crate that needs `sha2` (to content-address cons
//! cells) and `oxrdfio` (to read the Turtle back), and the arrangement surface needs neither.

use std::collections::{HashMap, HashSet};

use oxrdf::{NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use sha2::{Digest, Sha256};

use crate::{render_string_literal, Sexpr, SexprError, SexprResult, RDF_NS, SX_NS, XSD_STRING};

/// Datatype IRI marking a literal as a `Symbol` (distinct from `xsd:string`, so a symbol
/// round-trips as a symbol and never collapses into a string).
pub(crate) const SX_SYMBOL: &str = "https://ikigai-rs.dev/ns/sexpr#symbol";

/// Predicate on `<urn:sexpr:document>` naming the top node/literal of the encoded datum.
pub(crate) const SX_ROOT: &str = "https://ikigai-rs.dev/ns/sexpr#root";

/// The fixed document node whose `sx:root` names the encoded datum's top.
pub(crate) const SEXPR_DOCUMENT: &str = "urn:sexpr:document";

/// Prefix for content-addressed cons-cell node IRIs (`urn:sexpr:<hex>`).
pub(crate) const SEXPR_NODE_PREFIX: &str = "urn:sexpr:";

/// `rdf:first` — a cons cell's head.
pub(crate) const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
/// `rdf:rest` — a cons cell's tail (a node IRI, or `rdf:nil`).
pub(crate) const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
/// `rdf:nil` — the empty list.
pub(crate) const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
/// The XSD `integer` datatype IRI — an `Int` atom's literal type.
pub(crate) const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// A guard against unbounded recursion when decoding a (possibly hostile) graph. The encoder
/// never nests this deep; a graph that does is rejected rather than overflowing the stack.
pub(crate) const MAX_DECODE_DEPTH: usize = 4096;

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
/// empty list, `rdf:nil`). Content-addressed: each suffix's IRI is the hex of its subtree hash.
///
/// The cons chain is walked ITERATIVELY along the tail (never recursing per element), so a
/// long flat list `(a a a …)` cannot overflow the stack. `emit_node` still recurses into each
/// element's own subtree, but that is bounded by nesting depth (the reader caps it), not list
/// length. The per-suffix hashes are precomputed bottom-up in one pass ([`suffix_hashes`]),
/// so the whole walk is linear rather than re-hashing each suffix.
fn emit_list(items: &[Sexpr], stmts: &mut Vec<String>, seen: &mut HashSet<String>) -> String {
    if items.is_empty() {
        return "rdf:nil".to_string();
    }
    let hashes = suffix_hashes(items);
    let node_term = |i: usize| format!("<{SEXPR_NODE_PREFIX}{}>", to_hex(&hashes[i]));
    for i in 0..items.len() {
        let iri = format!("{SEXPR_NODE_PREFIX}{}", to_hex(&hashes[i]));
        // A suffix already in `seen` means it — and its whole remaining tail — was emitted
        // earlier (structural sharing); stop rather than re-walking.
        if !seen.insert(iri) {
            break;
        }
        let node = node_term(i);
        let first = emit_node(&items[i], stmts, seen);
        let rest = if i + 1 == items.len() {
            "rdf:nil".to_string()
        } else {
            node_term(i + 1)
        };
        stmts.push(format!("{node} rdf:first {first} ."));
        stmts.push(format!("{node} rdf:rest {rest} ."));
    }
    node_term(0)
}

/// A typed Turtle literal `"escaped-value"^^dt` — reusing [`render_string_literal`] so the
/// lexical form is escaped, never interpolated (a symbol/string can't break out of the graph).
fn typed_literal(value: &str, datatype_qname: &str) -> String {
    format!("{}^^{datatype_qname}", render_string_literal(value))
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
/// `H(0x04 ‖ hash(first) ‖ hash(rest))` — so the whole subtree determines the node IRI. The
/// fold over the tail is iterative (see [`suffix_hashes`]), so a long flat list never recurses
/// per element.
fn list_hash(items: &[Sexpr]) -> [u8; 32] {
    suffix_hashes(items)[0]
}

/// The content hash of every suffix of `items`, bottom-up: `out[i]` hashes `items[i..]` and
/// `out[len]` is the `nil` marker. Computed in a single reverse pass — no recursion along the
/// cons tail — so a long flat list is linear and stack-safe. Each `content_hash` still recurses
/// into an element's own subtree, but that is bounded by nesting depth, not list length.
fn suffix_hashes(items: &[Sexpr]) -> Vec<[u8; 32]> {
    let mut out = vec![[0u8; 32]; items.len() + 1];
    out[items.len()] = {
        let mut h = Sha256::new();
        h.update([0x00]); // nil marker
        h.finalize().into()
    };
    for i in (0..items.len()).rev() {
        let fh = content_hash(&items[i]);
        let mut h = Sha256::new();
        h.update([0x04]); // cons marker
        h.update(fh);
        h.update(out[i + 1]);
        out[i] = h.finalize().into();
    }
    out
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
