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
