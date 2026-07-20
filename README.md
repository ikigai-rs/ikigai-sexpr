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

## Using it from a host

```rust,ignore
let space = ikigai_sexpr::space(); // binds all four endpoints
// mount into your kernel alongside the SPARQL/RDF modules
```

`Sexpr`, the reader/printer, and the compilers are wasm-clean; the endpoints are
the only part that touches `ikigai-core`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
