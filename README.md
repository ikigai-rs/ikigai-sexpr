# ikigai-sexpr

The neutral **s-expression foundation** for [ikigai](https://github.com/ikigai-rs):
one datum that is queries, graphs, *and* code — homoiconic in a Lisp and
language-agnostic through transreptors. Every Lisp adapts *into* this datum; every
transreptor reads it as text, with no Lisp engine at all.

The core is pure Rust with no kernel dependency — a small `Sexpr` type, a
reader/printer, and the compilers — wrapped by ikigai endpoints that expose them
as first-class `ik:Transreptor`s.

```rust,ignore
pub enum Sexpr { Symbol(String), Str(String), Int(i64), List(Vec<Sexpr>) }
pub fn parse(&str) -> SexprResult<Sexpr>;          // text  -> datum
pub fn write(&Sexpr) -> String;                    // datum -> text
```

## The six surfaces

| endpoint | transreption | declared | what it does |
|---|---|---|---|
| `urn:sparql:from-sexpr` | `text/x-sexpr → application/sparql-query` | lossy | a **SELECT query** as an s-expr → SPARQL |
| `urn:rdf:from-sexpr` | `text/x-sexpr → text/turtle` | lossy | an **RDF graph** as an s-expr → Turtle (author graphs) |
| `urn:sexpr:to-rdf` | `text/x-sexpr → text/turtle` (code-graph profile) | lossless | **any s-expr → a lossless, content-addressed RDF graph** (put code in the fabric) |
| `urn:sexpr:from-rdf` | `text/turtle → text/x-sexpr` | lossy | the inverse of `to-rdf`, exact on a code-graph |
| `urn:sexpr:arrangement-to-rdf` | `text/x-ikigai-arrangement → text/turtle` | lossless | a **kernel's arrangement** as an s-expr → the `ik:` Turtle core builds a space from |
| `urn:sexpr:arrangement-from-rdf` | `text/turtle → text/x-ikigai-arrangement` | lossless | the exact inverse: an arrangement's Turtle → its canonical s-expr |

**Why three are lossy.** A transreptor that says nothing is declared lossless (core
0.1.77), and a lossless-only planner routes through it as if the output were the same
resource in another form. `text/x-sexpr` names an s-expression *datum*, and the two
compilers interpret the datum under one profile rather than encoding it: clauses are put in
canonical order, `pfx` and `pfx:` are one prefix, a graph's triples are a set, and anything
that is not a `(select …)` or a `(graph …)` is refused. Distinct datums give one output,
and nothing maps it back. `urn:sexpr:from-rdf` reads *any* Turtle and skips every triple
that is not part of the code-graph — an extraction, so two different graphs give one
s-expression; it is exact on a code-graph itself. Call any of the three by name, or plan
through it with the caller's consent; a plan that does reports the step as lossy. The
code-graph encoder and the arrangement pair are lossless, and say so by saying nothing.

Each is backed by a pure function you can also call directly: `sexpr_to_sparql`,
`sexpr_to_turtle`, `sexpr_to_rdf`, `rdf_to_sexpr` (kernel-free), and
`arrangement_to_turtle`, `turtle_to_arrangement` (through `ikigai-core`'s `Topology`).

## Queries as s-expressions

```text
(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))
```
compiles to
```sparql
SELECT ?s ?p ?o WHERE { ?s ?p ?o . } LIMIT 10
```
Terms are validated and literals escaped — an IRI or string can never break out
and inject query syntax.

## Graphs as s-expressions

```text
(graph (prefix (ex "http://example.org/"))
  (ex:alice a ex:Person)
  (ex:alice ex:name "Alice"))
```
compiles to Turtle (skolemized — no blank nodes; `a` auto-binds `rdf:`).

## Code as a graph (lossless, content-addressed)

`urn:sexpr:to-rdf` encodes an arbitrary s-expr as an `rdf:List` graph whose cons
cells are **content-addressed** (each node IRI is a SHA-256 of its subtree — so
identical sub-expressions share a node and the graph is self-fingerprinting).
Atoms carry distinguishing datatypes (`^^sx:symbol` / `xsd:string` /
`xsd:integer`). `urn:sexpr:from-rdf` decodes it back **exactly**:

```text
(sink "urn:x" 42)
```
```turtle
<urn:sexpr:document> sx:root <urn:sexpr:b95…> .
<urn:sexpr:b95…> rdf:first "sink"^^sx:symbol   ; rdf:rest <urn:sexpr:18a…> .
<urn:sexpr:18a…> rdf:first "urn:x"^^xsd:string ; rdf:rest <urn:sexpr:c36…> .
<urn:sexpr:c36…> rdf:first "42"^^xsd:integer   ; rdf:rest rdf:nil .
```

Once code is a graph you can SPARQL over it, sign it (its content-hash is a stable
fingerprint), cache it, and ship it — the substrate for portable, verifiable code.

## Arrangements as s-expressions

A kernel's arrangement — which endpoints answer which names, in what order, behind
which fallbacks, mounts, aliases, limiters and levels — is a resource: core renders it
as `ik:` Turtle (`urn:kernel:topology`) and builds a live space from that Turtle
(`ikigai_core::build`). This crate writes the same arrangement as an s-expression, so an
operator can keep it in an `*.arrangement` file (`text/x-ikigai-arrangement`) and the
ikigai host reads it through a lossless transreption to Turtle (`--arrangement`).

```text
space := (endpoints [:id "iri"] door…)
       | (fallback [:id "iri"] space…)
       | (mount "prefix" [:id "iri"] space)
       | (alias [:id "iri"] [:max-hops n] rule… space)
       | (limit "family" [:id "iri"] [:match prefix|exact|template])
       | (level "iri" [:seals ("prefix"…)] [:namespace "prefix"] space)
       | (ref "iri")                  ; a named space declared earlier
door  := (door "pattern" endpoint [:match exact|template] [:confined space])
rule  := (exact "from" "to") | (prefix "from" "to")
```

A declaration arranges endpoints the host registered, by name, and never mints one.
Anonymous spaces are nesting; `:id` names one. A door's match kind is inferred (a `{`
means a template), and `:match` is written only when the inference is wrong. Whatever
core cannot build — an opaque peer, a closure rewrite, a chain — is refused with where
and why, never skipped.

A complete small arrangement, its Turtle, and the space core builds from it:

```rust
use std::sync::Arc;
use futures::executor::block_on;
use ikigai_core::{
    build, Capability, FnEndpoint, Iri, Kernel, Registry, ReprType, Representation, Request,
    Verb,
};
use ikigai_sexpr::{arrangement_to_topology, arrangement_to_turtle, turtle_to_arrangement};

let arrangement = r#"(fallback
  (limit "urn:personal:")
  (endpoints :id "urn:example:public"
    (door "urn:example:hello" hello)
    (door "urn:example:greet:{name}" greeter)))
"#;

let turtle = r#"@prefix ik: <https://ikigai-rs.dev/ns#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<urn:ikigai:space:_:1> a ik:Fallback ;
    ik:layers <urn:ikigai:space:_:1:layer:1> .
<urn:ikigai:space:_:1:layer:1> rdf:first <urn:ikigai:space:_:2> ;
    rdf:rest <urn:ikigai:space:_:1:layer:2> .
<urn:ikigai:space:_:1:layer:2> rdf:first <urn:example:public> ;
    rdf:rest rdf:nil .

<urn:ikigai:space:_:2> a ik:Limit ;
    ik:family "urn:personal:" ;
    ik:matchKind "prefix" .

<urn:example:public> a ik:EndpointSpace ;
    ik:pattern "urn:example:hello" ;
    ik:pattern "urn:example:greet:{name}" ;
    ik:doors <urn:example:public:doors:1> .
<urn:example:public:doors:1> rdf:first <urn:example:public:door:1> ;
    rdf:rest <urn:example:public:doors:2> .
<urn:example:public:doors:2> rdf:first <urn:example:public:door:2> ;
    rdf:rest rdf:nil .

<urn:example:public:door:1> a ik:Door ;
    ik:pattern "urn:example:hello" ;
    ik:matchKind "exact" ;
    ik:endpointName "hello" .

<urn:example:public:door:2> a ik:Door ;
    ik:pattern "urn:example:greet:{name}" ;
    ik:matchKind "template" ;
    ik:endpointName "greeter" .
"#;

// Both directions, and each reads its own output back before answering.
assert_eq!(arrangement_to_turtle(arrangement).unwrap(), turtle);
assert_eq!(turtle_to_arrangement(turtle).unwrap(), arrangement);

// The host registers its endpoints by name; the file arranges them.
let says = |name: &'static str| {
    Arc::new(FnEndpoint::new(name, move |_| {
        Ok(Representation::new(ReprType::new("text/plain"), name.as_bytes().to_vec()))
    }))
};
let mut registry = Registry::new();
registry.register(says("hello")).unwrap();
registry.register(says("greeter")).unwrap();
let kernel = Kernel::new(build(&arrangement_to_topology(arrangement).unwrap(), &registry).unwrap());
let get = |name: &str| {
    block_on(kernel.issue(Request::new(Verb::Source, Iri::parse(name).unwrap()), &Capability::root()))
};
assert_eq!(get("urn:example:greet:ada").unwrap().bytes, b"greeter");
assert!(get("urn:personal:diary").is_err()); // the limiter is a hole, not a door
```

**Lossless, and what that does not cover.** Both transreptors are declared lossless,
and it is checked: each reads its own output back and refuses to answer if it gets a
different arrangement. Everything the arrangement means survives — every space, door,
rule and name, and every order that is meaning. What the Turtle cannot carry is
presentation: **a comment in the source is the one thing the round trip cannot keep**,
along with layout and the choice between equivalent spellings (a quoted or bare
endpoint name, where an option sits, a `:match` the pattern already implies, a
restated space instead of a `ref`, alias rules written out of table order). Back from
Turtle, an arrangement prints in the canonical form above.

The media type is deliberately not `text/x-sexpr`: `urn:sexpr:to-rdf` already
transrepts that to Turtle (and `urn:rdf:from-sexpr` does with consent), so a planner could
hand the builder the wrong graph.

**Bounded by core's bounds, in core's units.** An arrangement is held to
`ikigai_core::MAX_DECLARATION_DEPTH` (48 spaces deep — a door's confined corridor is a level,
the door is not), `MAX_DECLARATION_NODES` (65,536 spaces, doors and alias rules) and
`MAX_DECLARATION_TEXT` (16 MiB), a named space counted again at every place it is used. A
Turtle document is core's own to refuse as it reads it; the s-expression reader counts the
same way as it reads, and both refuse with core's typed `DeclarationError::TooLarge`, in
`ArrangementError::declaration`, naming the same node. The full grammar and every refusal
are in the `arrangement` module docs.

## Conformance

The six endpoints pass [`ikigai-conformance`](https://github.com/ikigai-rs/ikigai-conformance)
(`tests/conformance.rs`): every input is typed, every id is kebab-case, every
declared face is the one served, every RDF face is skolemized and uses defined
terms, and all six are declared **pure** — each is a total function of the
document text it is handed, so a `.cacheable()` result with no golden thread is
right rather than a resource nothing can cut.

Two things the endpoints deliberately declare, both pinned by that test:

- **`content` is required, `in` is optional.** They are two spellings of one
  document (the pipe lands in `content`; `in=` is the named alternative), and
  `ArgSpec` has no "exactly one of" group — so one of the two declarations has to
  be untrue, and the choice is about which failure a caller can recover from.

  Declaring **both optional** reads better to a pre-flight, and it silently makes
  the endpoint unpipeable: the REPL routes a piped value into the one *required*
  by-value argument left unnamed, so with none required every stage fails with
  ``accepts multiple arguments (content, in); name one with `key=value` ``. That
  is what 0.1.3 first did, on all four endpoints it had then, and nothing caught it — the
  conformance suite calls the kernel directly and names `content` explicitly, so
  the engine's routing is never exercised.

  Declaring **`content` required** costs the other direction: a pre-flight over
  the manifold (`urn:kernel:validate`, an MCP tool schema) refuses an `in=`-only
  call this endpoint would have accepted. But the caller *sees* that refusal and
  can pipe instead, whereas an endpoint that has quietly left every pipeline
  offers nothing to recover from. So `content` is required until `ikigai-core`
  gains a one-of group for inputs (`ikigai-core-PENDING.md` §29), at which point
  both can be true at once.

  Supplying neither is a typed `MissingArgument`; a document that is not this
  endpoint's shape is a typed `InvalidArgument` naming the input you passed.
- **`sx:` is this crate's own namespace.** `urn:sexpr:to-rdf` serves
  `sx:root` / `sx:symbol` under `https://ikigai-rs.dev/ns/sexpr#`, defined and
  used entirely here — deliberately *not* part of the shared `ikigai-rs.dev/ns`
  vocabulary while the encoding settles, and nothing under `ik:` is invented for
  it.

## Using it from a host

```rust,ignore
let space = ikigai_sexpr::space(); // binds all six endpoints
// mount into your kernel alongside the SPARQL/RDF modules
```

A host — or a page in a browser — that only reads arrangements mounts the arrangement
pair alone, and builds the crate without the default `full` feature, so nothing else is
compiled:

```toml
ikigai-sexpr = { version = "0.1", default-features = false }
```
```rust,ignore
let space = ikigai_sexpr::arrangement_space(); // urn:sexpr:arrangement-to-rdf / -from-rdf
```

`full` (on by default) is the code-graph codec (`sexpr_to_rdf`, `rdf_to_sexpr`), the four
s-expression endpoints and `space()`; it is what brings `sha2` and `oxrdfio` (with its
RDF/XML and JSON-LD parsers). Without it you keep the datum, the reader/printer, the two
pure compilers and the arrangement surface. Measured on a wasm32 consumer (`opt-level = "z"`,
LTO): a kernel mounting `space()` is 391 KB gzip, mounting `arrangement_space()` 241 KB,
and an empty kernel 159 KB.

`Sexpr`, the reader/printer, and the compilers are wasm-clean; the endpoints and the
arrangement surface are the only parts that touch `ikigai-core`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
