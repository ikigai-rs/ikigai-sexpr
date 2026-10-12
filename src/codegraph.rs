//! **The lossless code-as-graph codec** — `&Sexpr` ⇄ RDF Turtle, kernel-free. Behind the
//! `full` feature: it is the one part of the crate that needs `sha2` (to content-address cons
//! cells) and `oxrdfio` (to read the Turtle back), and the arrangement surface needs neither.

use std::collections::{HashMap, HashSet};

use oxrdf::{NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use sha2::{Digest, Sha256};

use crate::{
    render_string_literal, Sexpr, SexprError, SexprResult, SizeBudget, MAX_SEXPR_DEPTH, RDF_NS,
    SX_NS, XSD_STRING,
};

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
///
/// Linear in the datum: every node is hashed once, in one post-order pass (`ListHashes`),
/// and the emit walk reuses those hashes at every level, so a list nested `d` lists deep costs
/// the same as the list alone (ledger #1332).
pub fn sexpr_to_rdf(sexpr: &Sexpr) -> SexprResult<String> {
    let mut stmts: Vec<String> = Vec::new();
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    let root = match sexpr {
        Sexpr::List(items) => emit_list(items, &ListHashes::of(items), &mut stmts, &mut seen),
        atom => atom_term(atom),
    };

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

/// The content hashes of one list, computed once and kept beside the datum for the emit walk:
/// `suffix[i]` hashes the cons chain `items[i..]` (`suffix[len]` is the `nil` marker), and
/// `lists` holds the same for each element that is itself a list, in order (an atom costs one
/// hash and nothing more).
///
/// The hash is deterministic and domain-separated, a pure function of the subtree, so equal
/// subtrees hash equally and share a node:
/// - an atom is `H(tag ‖ len as u64 LE ‖ payload)`, with tag `0x01` for a symbol, `0x02` for
///   a string and `0x03` for an integer (payload its `i64` LE bytes); the length prefix
///   removes any ambiguity between differently split payloads;
/// - the empty chain is `H(0x00)`, and a cons cell is `H(0x04 ‖ hash(first) ‖ hash(rest))`, so
///   a list's hash is its first suffix's.
///
/// No time or randomness: reproducible, and a node's IRI is `urn:sexpr:` and this hash in hex.
struct ListHashes {
    suffix: Vec<[u8; 32]>,
    lists: Vec<ListHashes>,
}

impl ListHashes {
    /// Hash `items` and every list inside it, post-order: each element's hash is computed
    /// once, before the suffixes that fold it in. The fold along the cons tail is a reverse
    /// loop, never a recursion per element, so a long flat list is stack-safe; recursion is
    /// into nested lists only, bounded by nesting depth (the reader caps it).
    fn of(items: &[Sexpr]) -> ListHashes {
        let mut lists: Vec<ListHashes> = Vec::new();
        let mut firsts: Vec<[u8; 32]> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                Sexpr::List(sub) => {
                    let hashes = ListHashes::of(sub);
                    firsts.push(hashes.suffix[0]);
                    lists.push(hashes);
                }
                atom => firsts.push(atom_hash(atom)),
            }
        }
        let mut suffix = vec![[0u8; 32]; items.len() + 1];
        suffix[items.len()] = {
            let mut h = Sha256::new();
            h.update([0x00]); // nil marker
            h.finalize().into()
        };
        for i in (0..items.len()).rev() {
            let mut h = Sha256::new();
            h.update([0x04]); // cons marker
            h.update(firsts[i]);
            h.update(suffix[i + 1]);
            suffix[i] = h.finalize().into();
        }
        ListHashes { suffix, lists }
    }
}

/// The typed Turtle literal for an atom. Never called with a list.
fn atom_term(atom: &Sexpr) -> String {
    match atom {
        Sexpr::Symbol(s) => typed_literal(s, "sx:symbol"),
        Sexpr::Str(s) => typed_literal(s, "xsd:string"),
        Sexpr::Int(n) => format!("\"{n}\"^^xsd:integer"),
        Sexpr::List(_) => unreachable!("atom_term is called with atoms only"),
    }
}

/// The content hash of an atom (see [`ListHashes`] for the scheme). Never called with a list.
fn atom_hash(atom: &Sexpr) -> [u8; 32] {
    match atom {
        Sexpr::Symbol(s) => tagged_hash(0x01, s.as_bytes()),
        Sexpr::Str(s) => tagged_hash(0x02, s.as_bytes()),
        Sexpr::Int(n) => tagged_hash(0x03, &n.to_le_bytes()),
        Sexpr::List(_) => unreachable!("atom_hash is called with atoms only"),
    }
}

/// `H(tag ‖ len as u64 LE ‖ payload)`.
fn tagged_hash(tag: u8, payload: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([tag]);
    h.update((payload.len() as u64).to_le_bytes());
    h.update(payload);
    h.finalize().into()
}

/// The `<urn:sexpr:…>` term of a cons cell with this hash.
fn node_term(hash: &[u8; 32]) -> String {
    format!("<{SEXPR_NODE_PREFIX}{}>", to_hex(hash))
}

/// Emit the cons chain for a list, returning its node term (`<urn:sexpr:…>` or, for the empty
/// list, `rdf:nil`) and appending each *new* cons cell's two statements to `stmts`. A cell
/// already in `seen` is not re-emitted: that is where structural sharing shows up, and since
/// a cell's hash covers its whole tail, the rest of the chain was emitted with it.
///
/// The chain is walked ITERATIVELY along the tail (never recursing per element), so a long flat
/// list `(a a a …)` cannot overflow the stack; the walk recurses into an element only when it
/// is a list, bounded by nesting depth. Every hash comes from `hashes`, computed once.
fn emit_list(
    items: &[Sexpr],
    hashes: &ListHashes,
    stmts: &mut Vec<String>,
    seen: &mut HashSet<[u8; 32]>,
) -> String {
    if items.is_empty() {
        return "rdf:nil".to_string();
    }
    let mut sub_hashes = hashes.lists.iter();
    for (i, item) in items.iter().enumerate() {
        if !seen.insert(hashes.suffix[i]) {
            break;
        }
        let node = node_term(&hashes.suffix[i]);
        let first = match item {
            Sexpr::List(sub) => {
                let sub_hashes = sub_hashes
                    .next()
                    .expect("ListHashes::of hashed every list element, in order");
                emit_list(sub, sub_hashes, stmts, seen)
            }
            atom => atom_term(atom),
        };
        let rest = if i + 1 == items.len() {
            "rdf:nil".to_string()
        } else {
            node_term(&hashes.suffix[i + 1])
        };
        stmts.push(format!("{node} rdf:first {first} ."));
        stmts.push(format!("{node} rdf:rest {rest} ."));
    }
    node_term(&hashes.suffix[0])
}

/// A typed Turtle literal `"escaped-value"^^dt` — reusing [`render_string_literal`] so the
/// lexical form is escaped, never interpolated (a symbol/string can't break out of the graph).
fn typed_literal(value: &str, datatype_qname: &str) -> String {
    format!("{}^^{datatype_qname}", render_string_literal(value))
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
///
/// Bounded exactly as [`parse`](crate::parse) is: refused past 512 nested lists, past
/// 1,048,576 decoded nodes, or past 32 MiB of decoded symbol and string text. A node the graph
/// shares is decoded, and counted, at every place it is used, so a small graph that denotes a
/// huge tree is refused as it expands, not after:
///
/// ```
/// use ikigai_sexpr::rdf_to_sexpr;
///
/// const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// // `n_i` is the list `(n_{i+1} n_{i+1})`, one shared node used twice: 64 levels of it is
/// // about 10 KB of Turtle denoting a tree of 2^64 leaves.
/// let mut ttl = "<urn:sexpr:document> <https://ikigai-rs.dev/ns/sexpr#root> <urn:sexpr:n0> .\n"
///     .to_string();
/// for i in 0..64 {
///     ttl += &format!(
///         "<urn:sexpr:n{i}> <{RDF}first> <urn:sexpr:n{j}> ; <{RDF}rest> <urn:sexpr:m{i}> .\n\
///          <urn:sexpr:m{i}> <{RDF}first> <urn:sexpr:n{j}> ; <{RDF}rest> <{RDF}nil> .\n",
///         j = i + 1
///     );
/// }
/// ttl += &format!("<urn:sexpr:n64> <{RDF}first> \"x\" ; <{RDF}rest> <{RDF}nil> .\n");
/// let refused = rdf_to_sexpr(&ttl).unwrap_err().to_string();
/// assert!(refused.contains("code-graph decodes to more than 1048576 s-expression nodes"));
/// ```
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
    let mut budget = SizeBudget::new("code-graph decodes to");
    decode_term(&root, &firsts, &rests, 0, &mut budget)
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
/// cons-node IRI → the list it heads. `depth` counts the lists that enclose `term`, as the
/// reader counts them, so a list is refused exactly where [`parse`](crate::parse) would refuse
/// it ([`MAX_SEXPR_DEPTH`]), and before the recursion that would overflow the stack. Every
/// node is charged to `budget` before it is built, in the reader's units (a list or an
/// integer is a node; a symbol or string is a node and its bytes), so a shared node is
/// counted at every place it is used and the decode is refused as the tree grows past the
/// crate's size bounds, never after.
fn decode_term(
    term: &RdfTerm,
    firsts: &HashMap<String, RdfTerm>,
    rests: &HashMap<String, RdfTerm>,
    depth: usize,
    budget: &mut SizeBudget,
) -> SexprResult<Sexpr> {
    match term {
        RdfTerm::Literal { value, datatype } => {
            budget.charge(if datatype == XSD_INTEGER {
                0
            } else {
                value.len()
            })?;
            decode_atom(value, datatype)
        }
        RdfTerm::Iri(_) if depth >= MAX_SEXPR_DEPTH => Err(SexprError::Parse(format!(
            "code-graph nests deeper than {MAX_SEXPR_DEPTH} lists (the decode limit, the \
             same as the reader's)"
        ))),
        RdfTerm::Iri(iri) => {
            budget.charge(0)?;
            if iri == RDF_NIL {
                Ok(Sexpr::List(Vec::new()))
            } else {
                decode_list(iri, firsts, rests, depth, budget)
            }
        }
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
    budget: &mut SizeBudget,
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
        items.push(decode_term(first, firsts, rests, depth + 1, budget)?);
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

/// The encoder as it was before ledger #1332, kept as a TEST ORACLE: it recomputes a list's
/// hashes at every level the list is nested in (cost nodes x depth), and the encoder above
/// must answer byte for byte what it answers. Test-only; never compiled into the crate.
#[cfg(test)]
pub(crate) mod reference {
    use super::*;

    pub(crate) fn sexpr_to_rdf(sexpr: &Sexpr) -> String {
        let mut stmts: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let root = emit_node(sexpr, &mut stmts, &mut seen);
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
        out
    }

    fn emit_node(sexpr: &Sexpr, stmts: &mut Vec<String>, seen: &mut HashSet<String>) -> String {
        match sexpr {
            Sexpr::Symbol(s) => typed_literal(s, "sx:symbol"),
            Sexpr::Str(s) => typed_literal(s, "xsd:string"),
            Sexpr::Int(n) => format!("\"{n}\"^^xsd:integer"),
            Sexpr::List(items) => emit_list(items, stmts, seen),
        }
    }

    fn emit_list(items: &[Sexpr], stmts: &mut Vec<String>, seen: &mut HashSet<String>) -> String {
        if items.is_empty() {
            return "rdf:nil".to_string();
        }
        let hashes = suffix_hashes(items);
        let node_term = |i: usize| format!("<{SEXPR_NODE_PREFIX}{}>", to_hex(&hashes[i]));
        for i in 0..items.len() {
            let iri = format!("{SEXPR_NODE_PREFIX}{}", to_hex(&hashes[i]));
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

    fn content_hash(sexpr: &Sexpr) -> [u8; 32] {
        match sexpr {
            Sexpr::Symbol(s) => atom_hash(0x01, s.as_bytes()),
            Sexpr::Str(s) => atom_hash(0x02, s.as_bytes()),
            Sexpr::Int(n) => atom_hash(0x03, &n.to_le_bytes()),
            Sexpr::List(items) => suffix_hashes(items)[0],
        }
    }

    fn atom_hash(tag: u8, payload: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update([tag]);
        h.update((payload.len() as u64).to_le_bytes());
        h.update(payload);
        h.finalize().into()
    }

    fn suffix_hashes(items: &[Sexpr]) -> Vec<[u8; 32]> {
        let mut out = vec![[0u8; 32]; items.len() + 1];
        out[items.len()] = {
            let mut h = Sha256::new();
            h.update([0x00]);
            h.finalize().into()
        };
        for i in (0..items.len()).rev() {
            let fh = content_hash(&items[i]);
            let mut h = Sha256::new();
            h.update([0x04]);
            h.update(fh);
            h.update(out[i + 1]);
            out[i] = h.finalize().into();
        }
        out
    }
}
