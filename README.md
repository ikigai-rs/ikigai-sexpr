# ikigai-sexpr

The neutral **s-expression foundation** for [ikigai](https://github.com/ikigai-rs):
one datum that is queries, graphs, *and* code — homoiconic in a Lisp and
language-agnostic through transreptors. Every Lisp adapts *into* this datum; every
transreptor reads it as text, with no Lisp engine at all.

The core is pure Rust with no kernel dependency — a small `Sexpr` type, a
reader/printer, and four compilers — wrapped by ikigai endpoints that expose them
as first-class `ik:Transreptor`s.

```rust
pub enum Sexpr { Symbol(String), Str(String), Int(i64), List(Vec<Sexpr>) }
pub fn parse(&str) -> SexprResult<Sexpr>;          // text  -> datum
pub fn write(&Sexpr) -> String;                    // datum -> text
```

## The four surfaces

| endpoint | transreption | what it does |
|---|---|---|
| `urn:sparql:from-sexpr` | `text/x-sexpr → application/sparql-query` | a **SELECT query** as an s-expr → SPARQL |
| `urn:rdf:from-sexpr` | `text/x-sexpr → text/turtle` | an **RDF graph** as an s-expr → Turtle (author graphs) |
| `urn:sexpr:to-rdf` | `text/x-sexpr → text/turtle` (code-graph profile) | **any s-expr → a lossless, content-addressed RDF graph** (put code in the fabric) |
| `urn:sexpr:from-rdf` | `text/turtle → text/x-sexpr` | the exact inverse of `to-rdf` |

Each is backed by a pure, kernel-free function you can also call directly:
`sexpr_to_sparql`, `sexpr_to_turtle`, `sexpr_to_rdf`, `rdf_to_sexpr`.

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

## Conformance

The four endpoints pass [`ikigai-conformance`](https://github.com/ikigai-rs/ikigai-conformance)
(`tests/conformance.rs`): every input is typed, every id is kebab-case, every
declared face is the one served, both RDF faces are skolemized and use defined
terms, and all four are declared **pure** — each is a total function of the
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
  is what 0.1.3 first did, on all four endpoints, and nothing caught it — the
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
let space = ikigai_sexpr::space(); // binds all four endpoints
// mount into your kernel alongside the SPARQL/RDF modules
```

`Sexpr`, the reader/printer, and the compilers are wasm-clean; the endpoints are
the only part that touches `ikigai-core`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
