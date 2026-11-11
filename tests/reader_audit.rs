//! Regressions from the reader/printer and compiler audit (ledger #3428).
use ikigai_sexpr::{parse, parse_all, sexpr_to_sparql, sexpr_to_turtle, write, Sexpr};

const TEXT_BOUND: usize = 32 << 20;

#[test]
fn an_unterminated_string_stops_at_the_text_bound() {
    let input = format!("\"{}", "x".repeat(TEXT_BOUND + 1));
    let err = parse(&input).unwrap_err().to_string();
    assert!(err.contains("more than 33554432 bytes"), "{err}");
}

#[test]
fn a_numeric_symbol_round_trips() {
    let datum = Sexpr::Symbol("42".into());
    assert_eq!(parse(&write(&datum)).unwrap(), datum);
}

#[test]
fn a_leading_hyphen_in_a_local_name_is_refused() {
    for term in ["ex:-s", "ex:-", ":-s"] {
        let query = parse(&format!(
            "(select * (prefix (ex \"urn:ex:\") (: \"urn:default:\")) (where (?s ?p {term})))"
        ))
        .unwrap();
        assert!(sexpr_to_sparql(&query).is_err(), "accepted {term}");
        let graph = parse(&format!(
            "(graph (prefix (ex \"urn:ex:\") (: \"urn:default:\")) (ex:s ex:p {term}))"
        ))
        .unwrap();
        assert!(sexpr_to_turtle(&graph).is_err(), "accepted {term}");
    }
}

#[test]
fn malformed_language_tags_are_refused() {
    for tag in ["@123", "@-en", "@en-", "@en--US", "@en-_"] {
        let graph = parse(&format!(
            "(graph ((iri \"urn:s\") (iri \"urn:p\") (lit \"hi\" {tag})))"
        ))
        .unwrap();
        assert!(sexpr_to_turtle(&graph).is_err(), "accepted {tag}");
    }
}

#[test]
fn reader_datums_round_trip_including_all_ascii_and_unicode_strings() {
    let text: String = (0..=127)
        .map(char::from)
        .chain("é中🦀\u{2028}\u{2029}".chars())
        .collect();
    let datum = Sexpr::List(vec![
        Sexpr::Str(text),
        Sexpr::Int(i64::MIN),
        Sexpr::Int(i64::MAX),
    ]);
    assert_eq!(parse(&write(&datum)).unwrap(), datum);
    let forms = parse_all(
        "(:keyword 'quote `quasi ,unquote #| block |# + 9223372036854775808 -9223372036854775809)",
    )
    .unwrap();
    for form in forms {
        assert_eq!(parse(&write(&form)).unwrap(), form);
    }
}

#[test]
fn quoted_symbols_preserve_empty_delimited_and_numeric_names() {
    let mut names = vec![
        "".to_string(),
        "42".into(),
        "+42".into(),
        "-0".into(),
        "a b".into(),
        "a;b".into(),
        "(x)".into(),
        "a\"b\\c\n\tπ".into(),
        "#\"x\"".into(),
        "#".into(),
        ":keyword".into(),
        "#t".into(),
    ];
    names.extend((0..=127).map(|c| char::from(c).to_string()));
    let datum = Sexpr::List(names.into_iter().map(Sexpr::Symbol).collect());
    assert_eq!(parse(&write(&datum)).unwrap(), datum);
    assert_eq!(write(&Sexpr::Symbol("42".into())), "#\"42\"");
    assert_eq!(
        parse_all("#\"42\" \"42\" 42 # \"x\"").unwrap(),
        vec![
            Sexpr::Symbol("42".into()),
            Sexpr::Str("42".into()),
            Sexpr::Int(42),
            Sexpr::Symbol("#".into()),
            Sexpr::Str("x".into()),
        ]
    );
    for input in ["#\"", "#\"x\\", "#\"x\\\""] {
        assert!(parse(input).is_err(), "accepted unterminated {input:?}");
    }
}

#[test]
fn decoded_text_is_bounded_across_forms_in_utf8_bytes() {
    // Two bytes per character, including when an unknown escape drops the backslash.
    let half = "é".repeat(TEXT_BOUND / 4);
    let input = format!("\"{half}\" #\"{half}\"");
    assert_eq!(parse_all(&input).unwrap().len(), 2, "exactly the limit");
    for suffix in [" \"x\"", " #\"\\é\"", " x"] {
        let err = parse_all(&(input.clone() + suffix))
            .unwrap_err()
            .to_string();
        assert!(err.contains("more than 33554432 bytes"), "{err}");
    }
}

#[test]
fn the_node_budget_covers_the_entire_stream_and_precedes_string_allocation() {
    let mut input = "\"\" ".repeat(1 << 20);
    assert_eq!(parse_all(&input).unwrap().len(), 1 << 20);
    input.push_str("\"unterminated");
    let err = parse_all(&input).unwrap_err().to_string();
    assert!(
        err.contains("more than 1048576 s-expression nodes"),
        "{err}"
    );
}

// The quote/comment portion of ikigai-ui's sexpr::bound at
// a4c77c1bb57a3cf5494fd9f7c6bcb8f289149de9, src/sexpr.rs:111-145.
// Its 64 KiB source limit and 32-list limit are tighter than this crate's bounds.
fn prescan_depth(text: &str) -> usize {
    let mut depth = 0usize;
    let mut maximum = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            ';' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '(' => {
                depth += 1;
                maximum = maximum.max(depth);
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    maximum
}

fn datum_depth(forms: &[Sexpr]) -> usize {
    let mut stack: Vec<_> = forms.iter().map(|form| (form, 0)).collect();
    let mut maximum = 0;
    while let Some((form, depth)) = stack.pop() {
        if let Sexpr::List(items) = form {
            maximum = maximum.max(depth + 1);
            stack.extend(items.iter().map(|item| (item, depth + 1)));
        }
    }
    maximum
}

#[test]
fn quote_and_comment_scanning_agrees_with_the_ui_prescan() {
    // Exhaust all strings up to six characters over the syntax that could hide nesting.
    let alphabet = ['(', ')', '"', '\\', ';', '\n', 'a', '#'];
    for len in 0..=6 {
        for mut bits in 0usize..8usize.pow(len) {
            let mut input = String::new();
            for _ in 0..len {
                input.push(alphabet[bits % 8]);
                bits /= 8;
            }
            if let Ok(forms) = parse_all(&input) {
                assert_eq!(datum_depth(&forms), prescan_depth(&input), "{input:?}");
            }
        }
    }
    // Longer cases include escape parity, Unicode, unknown escapes, and quoted symbols.
    for text in [
        "\\\";((((\n)🦀",
        "\\\\\";)))))",
        "#| (not a block comment) |#",
    ] {
        for atom in [Sexpr::Str(text.into()), Sexpr::Symbol(text.into())] {
            let input = format!("; (((\n({} ; )))\n(a))", write(&atom));
            let forms = parse_all(&input).unwrap();
            assert_eq!(datum_depth(&forms), 2);
            assert_eq!(prescan_depth(&input), 2);
        }
    }
}

const DEPTH_PROBE: &str = "IKIGAI_SEXPR_READER_AUDIT_DEPTH";

#[test]
fn reader_depth_probe_child() {
    if std::env::var_os(DEPTH_PROBE).is_none() {
        return;
    }
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            for n in [32, 512, 513, 100_000] {
                let input = format!(
                    "{}#\"\\\";{}\"{}",
                    "(".repeat(n),
                    "(".repeat(10_000),
                    ")".repeat(n)
                );
                if n <= 512 {
                    let forms = parse_all(&input).unwrap();
                    assert_eq!(datum_depth(&forms), n);
                    assert_eq!(prescan_depth(&input), n);
                    let printed = write(&forms[0]);
                    // Equality, printing, and dropping also run on the 2 MiB thread.
                    assert_eq!(parse(&printed).unwrap(), forms[0]);
                } else {
                    let err = parse_all(&input).unwrap_err().to_string();
                    assert!(err.contains("reader limit (512)"), "{err}");
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_reader_cases_survive_a_two_mib_stack_in_a_child_process() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reader_depth_probe_child", "--nocapture"])
        .env(DEPTH_PROBE, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "full")]
mod rdf {
    use super::*;
    use oxigraph::sparql::SparqlEvaluator;
    use oxrdfio::{RdfFormat, RdfParser};

    fn turtle_is_valid(text: &str) -> bool {
        RdfParser::from_format(RdfFormat::Turtle)
            .for_slice(text.as_bytes())
            .all(|q| q.is_ok())
    }

    #[test]
    fn the_refused_names_really_are_invalid_rdf_syntax() {
        assert!(!turtle_is_valid(
            "@prefix ex: <urn:ex:> . ex:s ex:p ex:-s ."
        ));
        assert!(SparqlEvaluator::new()
            .parse_query("PREFIX ex: <urn:ex:> SELECT * WHERE { ?s ?p ex:-s }")
            .is_err());
        for tag in ["@123", "@-en", "@en-", "@en--US"] {
            assert!(
                !turtle_is_valid(&format!("<urn:s> <urn:p> \"hi\"{tag} .")),
                "{tag}"
            );
        }
    }

    #[test]
    fn valid_local_names_and_language_tags_still_parse_in_rdf_engines() {
        for local in ["", "s", "0", "_", "a-b", "0-", "_a"] {
            let query = parse(&format!(
                "(select * (prefix (ex \"urn:ex:\")) (where (?s ?p ex:{local})))"
            ))
            .unwrap();
            let compiled = sexpr_to_sparql(&query).unwrap();
            let _ = SparqlEvaluator::new().parse_query(&compiled).unwrap();
            let graph = parse(&format!(
                "(graph (prefix (ex \"urn:ex:\")) (ex:s ex:p ex:{local}))"
            ))
            .unwrap();
            assert!(turtle_is_valid(&sexpr_to_turtle(&graph).unwrap()));
        }
        for tag in ["@en", "@en-US", "@es-419", "@zh-Hant-TW", "@x-private"] {
            let graph = parse(&format!(
                "(graph ((iri \"urn:s\") (iri \"urn:p\") (lit \"hi\" {tag})))"
            ))
            .unwrap();
            assert!(turtle_is_valid(&sexpr_to_turtle(&graph).unwrap()), "{tag}");
        }
    }

    #[test]
    fn iri_delimiters_are_refused_in_both_compilers() {
        for c in [
            '<', '>', '"', '{', '}', '|', '^', '`', '\\', ' ', '\t', '\n', '\r', '\0',
        ] {
            let iri = write(&Sexpr::Str(format!("urn:x:{c}payload")));
            for query in [
                format!("(select * (where (?s ?p (iri {iri}))))"),
                format!("(select * (prefix (ex {iri})) (where (?s ?p ?o)))"),
            ] {
                assert!(sexpr_to_sparql(&parse(&query).unwrap()).is_err(), "{c:?}");
            }
            for graph in [
                format!("(graph ((iri \"urn:s\") (iri \"urn:p\") (iri {iri})))"),
                format!("(graph (prefix (ex {iri})))"),
                format!("(graph ((iri \"urn:s\") (iri \"urn:p\") (lit \"x\" (iri {iri}))))"),
            ] {
                assert!(sexpr_to_turtle(&parse(&graph).unwrap()).is_err(), "{c:?}");
            }
        }
    }

    #[test]
    fn literal_payloads_survive_both_compilers_as_one_unchanged_value() {
        use oxigraph::sparql::QueryResults;
        use oxigraph::store::Store;
        for payload in [
            "\" . <urn:extra> <urn:p> <urn:o> . #\n\\",
            "\" } UNION { ?s ?p ?o } #\n; ( ) 🦀",
            "nul\0backspace\u{8}formfeed\u{c}tab\treturn\rline\n",
        ] {
            let literal = write(&Sexpr::Str(payload.into()));
            let graph = parse(&format!(
                "(graph ((iri \"urn:s\") (iri \"urn:p\") {literal}))"
            ))
            .unwrap();
            let turtle = sexpr_to_turtle(&graph).unwrap();
            let triples = RdfParser::from_format(RdfFormat::Turtle)
                .for_slice(turtle.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(triples.len(), 1);
            let oxigraph::model::Term::Literal(value) = &triples[0].object else {
                panic!("expected a literal");
            };
            assert_eq!(value.value(), payload);
            let store = Store::new().unwrap();
            store
                .load_from_slice(RdfParser::from_format(RdfFormat::Turtle), turtle.as_bytes())
                .unwrap();
            let query = parse(&format!(
                "(select (?s) (where (?s (iri \"urn:p\") {literal})))"
            ))
            .unwrap();
            let compiled = sexpr_to_sparql(&query).unwrap();
            let results = SparqlEvaluator::new()
                .parse_query(&compiled)
                .unwrap()
                .on_store(&store)
                .execute()
                .unwrap();
            let QueryResults::Solutions(solutions) = results else {
                panic!("expected SELECT");
            };
            let solutions = solutions.collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(solutions.len(), 1);
            assert_eq!(solutions[0].get("s").unwrap().to_string(), "<urn:s>");
        }
    }

    #[test]
    fn code_graph_symbols_keep_their_type_through_text() {
        for text in ["42", "", "a b", "x;comment", "a\"b\\c\nπ"] {
            let datum = Sexpr::Symbol(text.into());
            let graph = ikigai_sexpr::sexpr_to_rdf(&datum).unwrap();
            let decoded = ikigai_sexpr::rdf_to_sexpr(&graph).unwrap();
            assert_eq!(parse(&write(&decoded)).unwrap(), datum);
        }
    }
}
