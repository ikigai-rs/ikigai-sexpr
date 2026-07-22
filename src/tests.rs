//! Tests for `ikigai-sexpr`: the reader/printer, the pure compiler (mirroring
//! ikigai-lisp's 9 compiler oracles over `Sexpr`), and the transreptor end-to-end
//! through the real `ikigai-sparql` engine.

use super::*;

// ---- the reader / printer ---------------------------------------------------

#[test]
fn reads_a_query_form_into_the_expected_datum() {
    let q = parse("(select (?s ?p ?o) (where (?s ?p ?o)))").unwrap();
    assert_eq!(
        q,
        Sexpr::List(vec![
            Sexpr::Symbol("select".into()),
            Sexpr::List(vec![
                Sexpr::Symbol("?s".into()),
                Sexpr::Symbol("?p".into()),
                Sexpr::Symbol("?o".into()),
            ]),
            Sexpr::List(vec![
                Sexpr::Symbol("where".into()),
                Sexpr::List(vec![
                    Sexpr::Symbol("?s".into()),
                    Sexpr::Symbol("?p".into()),
                    Sexpr::Symbol("?o".into()),
                ]),
            ]),
        ])
    );
}

#[test]
fn classifies_atoms_symbol_vs_integer() {
    // `a`, `*`, `?v`, `pfx:local`, `-3` stay symbols; a bare integer becomes Int.
    assert_eq!(parse("a").unwrap(), Sexpr::Symbol("a".into()));
    assert_eq!(parse("*").unwrap(), Sexpr::Symbol("*".into()));
    assert_eq!(parse("?v").unwrap(), Sexpr::Symbol("?v".into()));
    assert_eq!(parse("rdf:type").unwrap(), Sexpr::Symbol("rdf:type".into()));
    assert_eq!(parse("42").unwrap(), Sexpr::Int(42));
    assert_eq!(parse("-7").unwrap(), Sexpr::Int(-7));
}

#[test]
fn skips_comments_and_whitespace() {
    let q = parse("  ; a comment\n (limit  \n 5) ; trailing\n").unwrap();
    assert_eq!(
        q,
        Sexpr::List(vec![Sexpr::Symbol("limit".into()), Sexpr::Int(5)])
    );
}

#[test]
fn printer_round_trips_canonical_input() {
    // parse ∘ write is stable on canonical text (single spaces, canonical string escapes).
    for src in [
        "(select (?s ?p ?o) (where (?s ?p ?o)))",
        "(select * (where ((iri \"http://ex/s\") a ?o)) (order-by ?o (desc ?o)) (limit 10))",
        "(prefix (rdf \"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"))",
    ] {
        assert_eq!(write(&parse(src).unwrap()), src, "round-trip failed: {src}");
    }
}

#[test]
fn string_escapes_read_and_print_symmetrically() {
    // A literal carrying a quote, a backslash, and a newline round-trips through the
    // reader's decode and the printer's escape.
    let read = parse(r#"(x "a\"b\\c\nd")"#).unwrap();
    assert_eq!(
        read,
        Sexpr::List(vec![
            Sexpr::Symbol("x".into()),
            Sexpr::Str("a\"b\\c\nd".into()),
        ])
    );
    assert_eq!(write(&read), r#"(x "a\"b\\c\nd")"#);
}

#[test]
fn unbalanced_parens_are_errors() {
    assert!(parse("(select (?s").is_err(), "missing close paren");
    assert!(parse(")").is_err(), "stray close paren");
    assert!(parse("(a) (b)").is_err(), "trailing form after the first");
    assert!(parse("").is_err(), "empty input");
    assert!(
        parse(r#"(x "unterminated)"#).is_err(),
        "unterminated string"
    );
}

#[test]
fn deeply_nested_input_is_a_clean_error_not_a_stack_overflow() {
    // A pathological run of open parens must be rejected as a parse error, NOT abort the
    // process by overflowing the reader's recursion. (Before the depth guard this crashed.)
    let bomb = "(".repeat(100_000);
    let err = parse(&bomb).expect_err("a 100k-deep input must be rejected, not overflow");
    match err {
        SexprError::Parse(msg) => assert!(
            msg.contains("nests deeper"),
            "expected a depth-limit parse error, got: {msg}"
        ),
        other => panic!("expected a Parse error, got {other:?}"),
    }
    // A balanced-but-very-deep input is likewise a clean error, never a panic.
    let balanced = format!("{}{}", "(".repeat(100_000), ")".repeat(100_000));
    assert!(
        matches!(parse(&balanced), Err(SexprError::Parse(_))),
        "a balanced 100k-deep input must be a clean parse error"
    );
}

// ---- the compiler: ikigai-lisp's 9 oracles, retyped over `Sexpr` ------------

#[test]
fn compiles_a_basic_select_with_one_triple() {
    let q = parse("(select (?s ?p ?o) (where (?s ?p ?o)))").unwrap();
    assert_eq!(
        sexpr_to_sparql(&q).unwrap(),
        "SELECT ?s ?p ?o WHERE {\n  ?s ?p ?o .\n}"
    );
}

#[test]
fn compiles_prefix_projection_and_limit() {
    // PREFIX line, a narrowed projection, a prefixed-name predicate, and LIMIT — emitted
    // in canonical order regardless of clause order in the s-expr.
    let q = parse(
        r#"(select (?s)
             (limit 5)
             (prefix (rdf "http://www.w3.org/1999/02/22-rdf-syntax-ns#"))
             (where (?s rdf:type ?o)))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_sparql(&q).unwrap(),
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         SELECT ?s WHERE {\n  ?s rdf:type ?o .\n}\nLIMIT 5"
    );
}

#[test]
fn compiles_star_projection_iri_term_and_order_by() {
    // `*` projection, an (iri "…") subject, the `a` predicate keyword, and an ORDER BY
    // mixing a bare var with a (desc …) condition.
    let q = parse(
        r#"(select *
             (where ((iri "http://example.org/s") a ?o))
             (order-by ?o (desc ?o)))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_sparql(&q).unwrap(),
        "SELECT * WHERE {\n  <http://example.org/s> a ?o .\n}\nORDER BY ?o DESC(?o)"
    );
}

#[test]
fn string_literals_and_iris_are_escaped_not_interpolated() {
    // Safety is the point vs string-building: a literal's quotes/backslashes are escaped,
    // and an IRI carrying query syntax is rejected outright (never emitted).
    assert_eq!(render_string_literal("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    assert!(
        render_iri("http://ex/x> . } INJECT {").is_err(),
        "an IRI with IRIREF-illegal characters must be rejected"
    );
}

#[test]
fn an_unknown_query_head_is_an_error_not_a_panic() {
    assert!(sexpr_to_sparql(&parse("(delete-everything)").unwrap()).is_err());
}

#[test]
fn malformed_queries_are_rejected() {
    // A non-list, an unknown clause, a bad triple arity, and a missing where — all clean.
    assert!(sexpr_to_sparql(&parse("42").unwrap()).is_err());
    assert!(sexpr_to_sparql(&parse("(select (?s) (drop-table (?s ?p ?o)))").unwrap()).is_err());
    assert!(sexpr_to_sparql(&parse("(select (?s) (where (?s ?p)))").unwrap()).is_err());
    assert!(sexpr_to_sparql(&parse("(select (?s))").unwrap()).is_err()); // no where clause
}

#[test]
fn an_injected_iri_symbol_term_is_rejected() {
    // A bare symbol that is neither ?var nor pfx:local is rejected (full IRIs must go in
    // the validated (iri "…") form) — the compiler never emits an unrecognized token.
    assert!(sexpr_to_sparql(&parse("(select (?s) (where (?s ?p injected)))").unwrap()).is_err());
}

#[test]
fn a_negative_limit_is_rejected() {
    assert!(
        sexpr_to_sparql(&parse("(select (?s) (where (?s ?p ?o)) (limit -1))").unwrap()).is_err()
    );
}

// ---- the compiler is pure / kernel-free (compile-time proof) ----------------

#[test]
fn compiler_signature_is_kernel_free() {
    // `sexpr_to_sparql` is `&Sexpr -> SexprResult<String>` — no ikigai-core type appears
    // in its signature. This binding wouldn't type-check otherwise.
    let f: fn(&Sexpr) -> SexprResult<String> = sexpr_to_sparql;
    let q = parse("(select (?s ?p ?o) (where (?s ?p ?o)))").unwrap();
    assert!(f(&q).is_ok());
}

// ---- the transreptor end-to-end, through the real SPARQL engine -------------

mod endpoint {
    use super::*;
    use futures::executor::block_on;
    use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Resolution, Scope, Space};
    use std::sync::Arc;

    /// A kernel binding the real `ikigai-sparql` module (all four query verbs) plus this
    /// crate's `urn:sparql:from-sexpr` transreptor — so a compiled query runs end-to-end.
    fn kernel() -> Kernel {
        let space = ikigai_sparql::space().bind(Exact::new("urn:sparql:from-sexpr"), FromSexpr);
        Kernel::new(Arc::new(space))
    }

    fn source(iri: &str, arg: (&str, &[u8])) -> String {
        let request = Request::new(Verb::Source, Iri::parse(iri).unwrap())
            .with_arg(arg.0, ArgRef::Inline(arg.1.to_vec()));
        String::from_utf8(
            block_on(kernel().issue(request, &Capability::root()))
                .unwrap()
                .bytes,
        )
        .unwrap()
    }

    #[test]
    fn from_sexpr_emits_the_expected_sparql() {
        // The endpoint reads the piped s-expr and emits exactly the canonical SPARQL.
        let sparql = source(
            "urn:sparql:from-sexpr",
            (
                "content",
                b"(select (?s ?p ?o) (where (?s ?p ?o)) (limit 1))",
            ),
        );
        assert_eq!(sparql, "SELECT ?s ?p ?o WHERE {\n  ?s ?p ?o .\n}\nLIMIT 1");
    }

    #[test]
    fn compiled_query_runs_through_ikigai_sparql() {
        // sexpr TEXT → from-sexpr → SPARQL → urn:sparql:select over the always-loaded
        // vocabulary graph → a non-empty solution set.
        let sparql = source(
            "urn:sparql:from-sexpr",
            (
                "content",
                b"(select (?s ?p ?o) (where (?s ?p ?o)) (limit 1))",
            ),
        );
        let results = source("urn:sparql:select", ("query", sparql.as_bytes()));
        assert!(
            results.contains("\"bindings\""),
            "expected sparql-results JSON, got: {results}"
        );
        assert!(
            results.contains("\"value\""),
            "expected a non-empty solution set, got: {results}"
        );
    }

    #[test]
    fn a_malformed_sexpr_is_a_clean_endpoint_error() {
        let request = Request::new(Verb::Source, Iri::parse("urn:sparql:from-sexpr").unwrap())
            .with_arg("content", ArgRef::Inline(b"(nonsense)".to_vec()));
        assert!(block_on(kernel().issue(request, &Capability::root())).is_err());
    }

    #[test]
    fn describes_itself_as_a_transreptor_with_the_argspec() {
        let request = Request::new(Verb::Meta, Iri::parse("urn:sparql:from-sexpr").unwrap());
        let Resolution::Hit(resolved) = space().resolve(&request, &Scope::empty()) else {
            panic!("urn:sparql:from-sexpr resolves");
        };
        let d = resolved.endpoint.describe();
        assert!(d.verbs.contains(&Verb::Source));
        assert!(d.verbs.contains(&Verb::Meta));

        // The declared transreption: text/x-sexpr → application/sparql-query.
        let t = d.transreption().expect("from-sexpr is an ik:Transreptor");
        assert_eq!(t.from, vec![MEDIA_SEXPR.to_string()]);
        assert_eq!(t.to, vec![MEDIA_SPARQL_QUERY.to_string()]);

        // `content` is the (required) piped input; `in` an optional alternative.
        let content = d
            .inputs
            .iter()
            .find(|a| a.name == "content")
            .expect("content");
        assert!(content.required);
        assert_eq!(content.class.as_deref(), Some(XSD_STRING));
        let in_arg = d.inputs.iter().find(|a| a.name == "in").expect("in");
        assert!(!in_arg.required);
    }
}

// ---- the graph compiler: sexpr → Turtle -------------------------------------

#[test]
fn compiles_a_basic_graph_to_exact_turtle() {
    // @prefix line, an IRI-string-literal triple, and an integer literal — one
    // newline-terminated `S P O .` per triple, in author order.
    let g = parse(
        r#"(graph
             (prefix (ex "http://example.org/"))
             (ex:alice ex:name "Alice")
             (ex:alice ex:age 42))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_turtle(&g).unwrap(),
        "@prefix ex: <http://example.org/> .\n\
         ex:alice ex:name \"Alice\" .\n\
         ex:alice ex:age 42 .\n"
    );
}

#[test]
fn a_predicate_becomes_rdf_type_and_auto_binds_rdf() {
    // `a` renders as rdf:type; the rdf: prefix is auto-bound (after the author's prefixes)
    // so the emitted Turtle is self-contained. The subject is a full (iri "…").
    let g = parse(
        r#"(graph
             (prefix (foaf "http://xmlns.com/foaf/0.1/"))
             ((iri "http://example.org/alice") a foaf:Person))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_turtle(&g).unwrap(),
        "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         <http://example.org/alice> rdf:type foaf:Person .\n"
    );
}

#[test]
fn an_author_declared_rdf_prefix_is_not_duplicated() {
    // If the author already binds rdf:, `a` reuses it — no duplicate @prefix line.
    let g = parse(
        r#"(graph
             (prefix (ex "http://example.org/") (rdf "http://www.w3.org/1999/02/22-rdf-syntax-ns#"))
             (ex:a a ex:Thing))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_turtle(&g).unwrap(),
        "@prefix ex: <http://example.org/> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         ex:a rdf:type ex:Thing .\n"
    );
}

#[test]
fn compiles_string_int_typed_and_lang_literals() {
    // The object literal forms: bare string, integer, (lit "v" pfx:dt) typed, (lit "v" @tag).
    let g = parse(
        r#"(graph
             (prefix (ex "http://example.org/") (xsd "http://www.w3.org/2001/XMLSchema#"))
             (ex:x ex:s "hi")
             (ex:x ex:n 7)
             (ex:x ex:d (lit "3.14" xsd:decimal))
             (ex:x ex:g (lit "Bonjour" @fr)))"#,
    )
    .unwrap();
    assert_eq!(
        sexpr_to_turtle(&g).unwrap(),
        "@prefix ex: <http://example.org/> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         ex:x ex:s \"hi\" .\n\
         ex:x ex:n 7 .\n\
         ex:x ex:d \"3.14\"^^xsd:decimal .\n\
         ex:x ex:g \"Bonjour\"@fr .\n"
    );
}

#[test]
fn an_unknown_graph_head_is_an_error_not_a_panic() {
    // The wrong top form (a query, or nonsense) is a clean error — this is how the two
    // text/x-sexpr transreptors disambiguate by head.
    assert!(sexpr_to_turtle(&parse("(select (?s) (where (?s ?p ?o)))").unwrap()).is_err());
    assert!(sexpr_to_turtle(&parse("(nonsense)").unwrap()).is_err());
    assert!(sexpr_to_turtle(&parse("42").unwrap()).is_err());
}

#[test]
fn blank_node_forms_are_rejected_with_a_skolemize_message() {
    // Skolemize: a `_:b` node (subject or object) is rejected, pointing at stable IRIs.
    let err = sexpr_to_turtle(
        &parse(r#"(graph (prefix (ex "http://example.org/")) (_:b ex:p ex:o))"#).unwrap(),
    )
    .unwrap_err();
    assert!(err.detail().contains("blank node"), "got: {err}");
    assert!(sexpr_to_turtle(
        &parse(r#"(graph (prefix (ex "http://example.org/")) (ex:s ex:p _:o))"#).unwrap()
    )
    .is_err());
}

#[test]
fn graph_terms_are_escaped_and_validated_not_interpolated() {
    // An IRI carrying Turtle-breaking syntax is rejected outright (never emitted); a literal
    // carrying a quote/newline/dot is escaped, not interpolated (proven below by re-parsing).
    assert!(
        sexpr_to_turtle(
            &parse(r#"(graph (ex:s ex:p (iri "http://x> . <urn:evil> a <urn:pwned")))"#).unwrap()
        )
        .is_err(),
        "an IRI with IRIREF-illegal characters must be rejected"
    );
    // A bare (unprefixed, non-IRI) subject symbol is rejected — never emitted as a raw token.
    assert!(sexpr_to_turtle(&parse("(graph (injected ex:p ex:o))").unwrap()).is_err());
    // A ?variable is not a graph node.
    assert!(sexpr_to_turtle(&parse("(graph (?s ex:p ex:o))").unwrap()).is_err());
    // A malformed (lit …) is a clean error, not a panic.
    assert!(sexpr_to_turtle(&parse(r#"(graph (ex:s ex:p (lit "v")))"#).unwrap()).is_err());
    assert!(sexpr_to_turtle(&parse(r#"(graph (ex:s ex:p (lit "v" @)))"#).unwrap()).is_err());
}

#[test]
fn the_signature_is_pure_and_kernel_free() {
    // `sexpr_to_turtle` is `&Sexpr -> SexprResult<String>` — no ikigai-core type appears.
    let f: fn(&Sexpr) -> SexprResult<String> = sexpr_to_turtle;
    assert!(f(&parse("(graph)").unwrap()).is_ok()); // an empty graph is valid (empty RDF)
}

// ---- Turtle VALIDITY: the emitted graph re-parses as RDF --------------------

mod validity {
    use super::*;
    use oxrdfio::{RdfFormat, RdfParser};

    /// Parse `ttl` as Turtle and render each resulting triple to its canonical N-Triples
    /// line (`S P O`). Panics if the Turtle is not valid RDF — so calling this at all is
    /// the validity assertion; the returned lines let a test assert the exact triples.
    fn triples(ttl: &str) -> Vec<String> {
        let mut out = Vec::new();
        for q in RdfParser::from_format(RdfFormat::Turtle).for_slice(ttl.as_bytes()) {
            let q = q.expect("emitted Turtle must be valid RDF");
            out.push(format!("{} {} {}", q.subject, q.predicate, q.object));
        }
        out
    }

    #[test]
    fn basic_graph_reparses_to_the_expected_triples() {
        let g = parse(
            r#"(graph
                 (prefix (ex "http://example.org/"))
                 (ex:alice ex:name "Alice")
                 (ex:alice ex:age 42))"#,
        )
        .unwrap();
        let lines = triples(&sexpr_to_turtle(&g).unwrap());
        assert_eq!(lines.len(), 2);
        assert!(lines.contains(
            &r#"<http://example.org/alice> <http://example.org/name> "Alice""#.to_string()
        ));
        assert!(lines.iter().any(|l| l.starts_with(
            "<http://example.org/alice> <http://example.org/age> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        )));
    }

    #[test]
    fn a_type_triple_reparses_with_the_expanded_rdf_type_iri() {
        let g = parse(
            r#"(graph
                 (prefix (foaf "http://xmlns.com/foaf/0.1/"))
                 ((iri "http://example.org/alice") a foaf:Person))"#,
        )
        .unwrap();
        let lines = triples(&sexpr_to_turtle(&g).unwrap());
        assert_eq!(
            lines,
            vec!["<http://example.org/alice> \
                 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                 <http://xmlns.com/foaf/0.1/Person>"
                .to_string()]
        );
    }

    #[test]
    fn an_injection_payload_literal_stays_one_triple() {
        // A literal carrying `"`, a newline, a `.`, and Turtle statement syntax must NOT
        // break out: the graph re-parses to exactly ONE triple, the payload trapped inside
        // the (escaped) literal. This is the escaping-not-interpolation guarantee, proven.
        let evil = "he said \"boom\" .\n<urn:x> a <urn:y> .";
        let g = Sexpr::List(vec![
            Sexpr::Symbol("graph".into()),
            Sexpr::List(vec![
                Sexpr::Symbol("prefix".into()),
                Sexpr::List(vec![
                    Sexpr::Symbol("ex".into()),
                    Sexpr::Str("http://example.org/".into()),
                ]),
            ]),
            Sexpr::List(vec![
                Sexpr::Symbol("ex:s".into()),
                Sexpr::Symbol("ex:p".into()),
                Sexpr::Str(evil.into()),
            ]),
        ]);
        let lines = triples(&sexpr_to_turtle(&g).unwrap());
        assert_eq!(lines.len(), 1, "the payload injected extra triples");
        let l = &lines[0];
        assert!(l.starts_with("<http://example.org/s> <http://example.org/p> \""));
        assert!(l.contains("boom"), "the literal text survived: {l}");
    }
}

// ---- the graph transreptor end-to-end, through the kernel -------------------

mod turtle_endpoint {
    use super::*;
    use futures::executor::block_on;
    use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Resolution, Scope, Space};
    use std::sync::Arc;

    /// A kernel over this crate's `space()` — which binds both from-sexpr transreptors.
    fn source(content: &[u8]) -> String {
        let kernel = Kernel::new(Arc::new(space()));
        let request = Request::new(Verb::Source, Iri::parse("urn:rdf:from-sexpr").unwrap())
            .with_arg("content", ArgRef::Inline(content.to_vec()));
        String::from_utf8(
            block_on(kernel.issue(request, &Capability::root()))
                .unwrap()
                .bytes,
        )
        .unwrap()
    }

    #[test]
    fn from_sexpr_emits_the_expected_turtle() {
        // sexpr graph TEXT piped in → text/turtle out, end-to-end through the kernel.
        let ttl =
            source(b"(graph (prefix (ex \"http://example.org/\")) (ex:alice ex:name \"Alice\"))");
        assert_eq!(
            ttl,
            "@prefix ex: <http://example.org/> .\nex:alice ex:name \"Alice\" .\n"
        );
    }

    #[test]
    fn a_query_form_is_rejected_by_the_graph_transreptor() {
        // Disambiguation in practice: a `(select …)` piped to urn:rdf:from-sexpr errors on
        // the wrong head (it wants a `(graph …)`), and vice-versa.
        let kernel = Kernel::new(Arc::new(space()));
        let req = Request::new(Verb::Source, Iri::parse("urn:rdf:from-sexpr").unwrap()).with_arg(
            "content",
            ArgRef::Inline(b"(select (?s) (where (?s ?p ?o)))".to_vec()),
        );
        assert!(block_on(kernel.issue(req, &Capability::root())).is_err());
    }

    #[test]
    fn describes_itself_as_a_turtle_transreptor() {
        let request = Request::new(Verb::Meta, Iri::parse("urn:rdf:from-sexpr").unwrap());
        let Resolution::Hit(resolved) = space().resolve(&request, &Scope::empty()) else {
            panic!("urn:rdf:from-sexpr resolves");
        };
        let d = resolved.endpoint.describe();
        assert!(d.verbs.contains(&Verb::Source));
        assert!(d.verbs.contains(&Verb::Meta));

        // The declared transreption: text/x-sexpr → text/turtle (distinct target type from
        // the sparql transreptor — that IS the primary disambiguator).
        let t = d
            .transreption()
            .expect("rdf-from-sexpr is an ik:Transreptor");
        assert_eq!(t.from, vec![MEDIA_SEXPR.to_string()]);
        assert_eq!(t.to, vec![MEDIA_TURTLE.to_string()]);

        let content = d
            .inputs
            .iter()
            .find(|a| a.name == "content")
            .expect("content");
        assert!(content.required);
        assert_eq!(content.class.as_deref(), Some(XSD_STRING));
        let in_arg = d.inputs.iter().find(|a| a.name == "in").expect("in");
        assert!(!in_arg.required);
    }
}

// ---- slice 3c.2: the LOSSLESS code-as-graph codec ---------------------------

mod code_graph {
    use super::*;
    use oxrdf::Term;
    use oxrdfio::{RdfFormat, RdfParser};
    use std::collections::HashSet;

    /// Parse code-graph Turtle into `(subject, predicate, object)` string triples. Subjects/
    /// objects are their IRIs; a literal object becomes `LIT[<datatype>]<value>`; a blank node
    /// becomes `_:…` (so the skolem test can detect one). Panics if the Turtle is not valid RDF.
    fn parse_ttl(ttl: &str) -> Vec<(String, String, String)> {
        use oxrdf::NamedOrBlankNode;
        let mut out = Vec::new();
        for q in RdfParser::from_format(RdfFormat::Turtle).for_slice(ttl.as_bytes()) {
            let q = q.expect("emitted code-graph must be valid RDF");
            let s = match q.subject {
                NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
                NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
            };
            let o = match q.object {
                Term::NamedNode(n) => n.as_str().to_string(),
                Term::Literal(l) => format!("LIT[{}]{}", l.datatype().as_str(), l.value()),
                Term::BlankNode(b) => format!("_:{}", b.as_str()),
            };
            out.push((s, q.predicate.as_str().to_string(), o));
        }
        out
    }

    fn assert_round_trips(s: &Sexpr) {
        let ttl = sexpr_to_rdf(s).unwrap();
        let back = rdf_to_sexpr(&ttl).unwrap();
        assert_eq!(
            &back, s,
            "round-trip failed for {s:?}\n--- turtle ---\n{ttl}"
        );
    }

    #[test]
    fn round_trips_the_full_corpus_exactly() {
        // Each atom kind; the empty list; nesting; a REPEATED sub-list (content-addressing
        // must still reconstruct both occurrences); and a real program (the 3a/3b shape).
        let corpus = vec![
            Sexpr::Symbol("select".into()),            // top-level symbol atom
            Sexpr::Str("hello \"world\"\n\tπ".into()), // string with escapes + unicode
            Sexpr::Int(42),                            // positive int
            Sexpr::Int(-7),                            // negative int
            Sexpr::List(Vec::new()),                   // ()
            parse("(a b c)").unwrap(),                 // flat list
            parse("(a (b (c) d) e)").unwrap(),         // nested
            parse("((x y) (x y))").unwrap(),           // repeated sub-list (dedup)
            parse(r#"(sink "urn:x" 42)"#).unwrap(),    // the brief's worked example
            parse("(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))").unwrap(), // a program
        ];
        for s in &corpus {
            assert_round_trips(s);
        }
    }

    #[test]
    fn a_long_flat_list_encodes_and_round_trips_without_overflow() {
        // The encoder used to recurse once per list ELEMENT (emit_list / list_hash on the
        // tail), so a flat list of tens of thousands of atoms overflowed the stack during
        // `sexpr_to_rdf`. The cons chain is now walked iteratively — this must complete and
        // reconstruct the exact datum.
        let n = 50_000;
        let items: Vec<Sexpr> = (0..n).map(|_| Sexpr::Symbol("a".into())).collect();
        let s = Sexpr::List(items);

        let ttl = sexpr_to_rdf(&s).expect("a long flat list must encode, not overflow");
        // Each SUFFIX hashes differently (its hash folds in the rest), so a flat list is a
        // full chain of N distinct cons cells — one `rdf:first` per element. (Sharing only
        // collapses identical SUB-LISTS, not repeated atoms in a chain.) The point of the
        // test is that emitting all N completes iteratively rather than overflowing.
        assert_eq!(
            ttl.matches(" rdf:first ").count(),
            n,
            "a flat N-element list must emit N cons cells"
        );

        let back = rdf_to_sexpr(&ttl).expect("the long list must decode back");
        assert_eq!(back, s, "a long flat list must round-trip exactly");
    }

    #[test]
    fn a_long_list_of_distinct_atoms_round_trips() {
        // Distinct elements ⇒ a distinct cons node per suffix (no sharing), exercising the
        // iterative walk across the full N-length chain rather than the dedup fast path.
        let n = 20_000;
        let items: Vec<Sexpr> = (0..n).map(Sexpr::Int).collect();
        let s = Sexpr::List(items);
        let ttl = sexpr_to_rdf(&s).expect("a long distinct-atom list must encode");
        let back = rdf_to_sexpr(&ttl).expect("it must decode back");
        assert_eq!(back, s, "a long distinct-atom list must round-trip exactly");
    }

    #[test]
    fn encoding_is_deterministic_byte_for_byte() {
        // Same datum → byte-identical Turtle, every time (content-addressed + sorted output).
        let s = parse("(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))").unwrap();
        let a = sexpr_to_rdf(&s).unwrap();
        let b = sexpr_to_rdf(&s).unwrap();
        assert_eq!(a, b);
        // Two independently-built EQUAL datums also encode identically (structural, not identity).
        let s2 = parse("(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))").unwrap();
        assert_eq!(sexpr_to_rdf(&s2).unwrap(), a);
    }

    #[test]
    fn a_repeated_sub_list_shares_one_content_addressed_node() {
        // `((x y) (x y))`: the two `(x y)` sub-lists MUST collapse to a single shared cons node
        // (its IRI is the hash of the subtree). Proves content-addressing / structural dedup.
        let s = parse("((x y) (x y))").unwrap();
        let ttl = sexpr_to_rdf(&s).unwrap();
        let quads = parse_ttl(&ttl);

        // Exactly four DISTINCT cons nodes: the two outer cells + the ONE shared `(x y)` head
        // + its `(y)` tail. Without sharing there would be six.
        let cons_subjects: HashSet<&String> = quads
            .iter()
            .map(|(subj, _, _)| subj)
            .filter(|subj| {
                subj.starts_with(SEXPR_NODE_PREFIX) && subj.as_str() != "urn:sexpr:document"
            })
            .collect();
        assert_eq!(
            cons_subjects.len(),
            4,
            "expected 4 distinct cons nodes (the two (x y) sharing one), got {cons_subjects:?}"
        );

        // The two outer cells' `rdf:first` both point at the SAME `(x y)` node IRI.
        let first_refs: Vec<&String> = quads
            .iter()
            .filter(|(_, p, o)| p == RDF_FIRST && o.starts_with(SEXPR_NODE_PREFIX))
            .map(|(_, _, o)| o)
            .collect();
        assert_eq!(first_refs.len(), 2, "two references to (x y) expected");
        assert_eq!(
            first_refs[0], first_refs[1],
            "the repeated sub-list must resolve to ONE shared IRI"
        );
    }

    #[test]
    fn the_code_graph_is_skolemized_no_blank_nodes() {
        let s = parse("(a (b c) (b c) () (d (e)))").unwrap();
        let ttl = sexpr_to_rdf(&s).unwrap();
        assert!(
            !ttl.contains("_:"),
            "no `_:` bnode syntax in the Turtle:\n{ttl}"
        );
        for (subj, _, obj) in parse_ttl(&ttl) {
            assert!(!subj.starts_with("_:"), "blank-node subject {subj}");
            assert!(!obj.starts_with("_:"), "blank-node object {obj}");
        }
    }

    #[test]
    fn atoms_carry_their_distinguishing_datatypes() {
        // A symbol and a string with the SAME text must NOT collapse — the datatype separates
        // them, so each round-trips to its own kind.
        let ttl_sym = sexpr_to_rdf(&Sexpr::Symbol("x".into())).unwrap();
        let ttl_str = sexpr_to_rdf(&Sexpr::Str("x".into())).unwrap();
        assert!(ttl_sym.contains("^^sx:symbol"), "{ttl_sym}");
        assert!(ttl_str.contains("^^xsd:string"), "{ttl_str}");
        assert_ne!(ttl_sym, ttl_str);
        assert_eq!(rdf_to_sexpr(&ttl_sym).unwrap(), Sexpr::Symbol("x".into()));
        assert_eq!(rdf_to_sexpr(&ttl_str).unwrap(), Sexpr::Str("x".into()));
    }

    #[test]
    fn the_worked_example_encodes_to_the_expected_shape() {
        // `(sink "urn:x" 42)` — assert the concrete triples, datatypes and the document root,
        // then that it round-trips.
        let s = parse(r#"(sink "urn:x" 42)"#).unwrap();
        let ttl = sexpr_to_rdf(&s).unwrap();
        let quads = parse_ttl(&ttl);

        // root marker
        assert!(quads
            .iter()
            .any(|(su, p, _)| su == "urn:sexpr:document" && p == SX_ROOT));
        // the three heads, by datatype
        let objs: Vec<&String> = quads
            .iter()
            .filter(|(_, p, _)| p == RDF_FIRST)
            .map(|(_, _, o)| o)
            .collect();
        assert!(
            objs.contains(&&format!("LIT[{SX_SYMBOL}]sink")),
            "objs: {objs:?}"
        );
        assert!(
            objs.contains(&&format!("LIT[{XSD_STRING}]urn:x")),
            "objs: {objs:?}"
        );
        assert!(
            objs.contains(&&format!("LIT[{XSD_INTEGER}]42")),
            "objs: {objs:?}"
        );
        // one tail is rdf:nil
        assert!(quads.iter().any(|(_, p, o)| p == RDF_REST && o == RDF_NIL));

        assert_eq!(rdf_to_sexpr(&ttl).unwrap(), s);
    }

    #[test]
    fn malformed_code_graphs_are_clean_errors_not_panics() {
        // No root marker.
        assert!(rdf_to_sexpr("@prefix ex: <http://e/> . ex:a ex:b ex:c .").is_err());
        // Not even RDF.
        assert!(rdf_to_sexpr("this is not turtle {{{").is_err());
        // Root points at a cons node with no rdf:first.
        let dangling = format!(
            "@prefix sx: <{SX_NS}> .\n<urn:sexpr:document> sx:root <urn:sexpr:missing> .\n"
        );
        assert!(rdf_to_sexpr(&dangling).is_err());
        // An unknown atom datatype.
        let bad_dt = format!(
            "@prefix sx: <{SX_NS}> .\n<urn:sexpr:document> sx:root \"x\"^^<http://ex/weird> .\n"
        );
        assert!(rdf_to_sexpr(&bad_dt).is_err());
    }

    #[test]
    fn other_triples_in_the_document_are_ignored() {
        // A code-graph embedded in a larger graph still decodes (unrelated triples are skipped).
        let s = parse("(a b)").unwrap();
        let mut ttl = sexpr_to_rdf(&s).unwrap();
        ttl.push_str("<http://example.org/note> <http://example.org/says> \"hi\" .\n");
        assert_eq!(rdf_to_sexpr(&ttl).unwrap(), s);
    }

    #[test]
    fn the_codec_signatures_are_pure_and_kernel_free() {
        // `sexpr_to_rdf` / `rdf_to_sexpr` carry no ikigai-core type — kernel-free.
        let enc: fn(&Sexpr) -> SexprResult<String> = sexpr_to_rdf;
        let dec: fn(&str) -> SexprResult<Sexpr> = rdf_to_sexpr;
        let s = parse("(a b)").unwrap();
        assert_eq!(dec(&enc(&s).unwrap()).unwrap(), s);
    }

    // ---- the govern payoff: a real SPARQL query OVER encoded code ----------------

    #[test]
    fn sparql_over_encoded_code_finds_the_head_symbol() {
        use oxigraph::sparql::{QueryResults, SparqlEvaluator};
        use oxigraph::store::Store;

        // Encode a program, load its code-graph into a triple store, and ask SPARQL for the
        // head symbol — code is now a queryable/governable graph, not opaque text.
        let program = parse("(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))").unwrap();
        let ttl = sexpr_to_rdf(&program).unwrap();

        let store = Store::new().unwrap();
        store
            .load_from_slice(
                oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::Turtle),
                ttl.as_bytes(),
            )
            .expect("code-graph loads into oxigraph");

        let query = format!(
            "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
             PREFIX sx: <{SX_NS}>\n\
             SELECT ?head WHERE {{ <urn:sexpr:document> sx:root ?top . ?top rdf:first ?head . }}"
        );
        let results = SparqlEvaluator::new()
            .parse_query(&query)
            .unwrap()
            .on_store(&store)
            .execute()
            .unwrap();

        let mut heads = Vec::new();
        if let QueryResults::Solutions(solutions) = results {
            for sol in solutions {
                let sol = sol.unwrap();
                if let Some(oxigraph::model::Term::Literal(l)) = sol.get("head") {
                    heads.push(l.value().to_string());
                }
            }
        }
        assert_eq!(
            heads,
            vec!["select".to_string()],
            "SPARQL found the head symbol"
        );
    }
}

// ---- slice 3c.2: the to-rdf / from-rdf endpoints, end-to-end through the kernel ----

mod code_graph_endpoints {
    use super::*;
    use futures::executor::block_on;
    use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Resolution, Scope, Space};
    use std::sync::Arc;

    fn source(iri: &str, content: &[u8]) -> String {
        let kernel = Kernel::new(Arc::new(space()));
        let request = Request::new(Verb::Source, Iri::parse(iri).unwrap())
            .with_arg("content", ArgRef::Inline(content.to_vec()));
        String::from_utf8(
            block_on(kernel.issue(request, &Capability::root()))
                .unwrap()
                .bytes,
        )
        .unwrap()
    }

    #[test]
    fn to_rdf_then_from_rdf_round_trips_through_the_kernel() {
        // sexpr TEXT → urn:sexpr:to-rdf → code-graph Turtle → urn:sexpr:from-rdf → sexpr TEXT.
        let input = "(select (?s ?p ?o) (where (?s ?p ?o)) (limit 10))";
        let ttl = source("urn:sexpr:to-rdf", input.as_bytes());
        assert!(ttl.contains("sx:root"), "code-graph turtle: {ttl}");
        let back = source("urn:sexpr:from-rdf", ttl.as_bytes());
        // Canonical text is stable: write(parse(input)) == input for this input.
        assert_eq!(back, input);
    }

    #[test]
    fn to_rdf_describes_itself_with_the_code_graph_profile() {
        let request = Request::new(Verb::Meta, Iri::parse("urn:sexpr:to-rdf").unwrap());
        let Resolution::Hit(resolved) = space().resolve(&request, &Scope::empty()) else {
            panic!("urn:sexpr:to-rdf resolves");
        };
        let d = resolved.endpoint.describe();
        assert!(d.verbs.contains(&Verb::Source) && d.verbs.contains(&Verb::Meta));
        let t = d.transreption().expect("to-rdf is an ik:Transreptor");
        assert_eq!(t.from, vec![MEDIA_SEXPR.to_string()]);
        // The distinguishing target: text/turtle WITH the code-graph profile (not plain
        // text/turtle) — that is how urn:transrept:auto tells it from urn:rdf:from-sexpr.
        assert_eq!(t.to, vec![MEDIA_TURTLE_CODE_GRAPH.to_string()]);
        assert!(t.to[0].contains("profile="));
        assert_ne!(t.to[0], MEDIA_TURTLE, "must NOT collide with domain Turtle");
    }

    #[test]
    fn from_rdf_describes_itself_as_turtle_to_sexpr() {
        let request = Request::new(Verb::Meta, Iri::parse("urn:sexpr:from-rdf").unwrap());
        let Resolution::Hit(resolved) = space().resolve(&request, &Scope::empty()) else {
            panic!("urn:sexpr:from-rdf resolves");
        };
        let d = resolved.endpoint.describe();
        let t = d.transreption().expect("from-rdf is an ik:Transreptor");
        assert_eq!(t.from, vec![MEDIA_TURTLE.to_string()]);
        assert_eq!(t.to, vec![MEDIA_SEXPR.to_string()]);
    }

    #[test]
    fn a_malformed_document_is_a_clean_endpoint_error() {
        let kernel = Kernel::new(Arc::new(space()));
        let req = Request::new(Verb::Source, Iri::parse("urn:sexpr:from-rdf").unwrap())
            .with_arg("content", ArgRef::Inline(b"not a code-graph".to_vec()));
        assert!(block_on(kernel.issue(req, &Capability::root())).is_err());
    }
}
