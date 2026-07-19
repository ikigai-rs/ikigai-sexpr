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
