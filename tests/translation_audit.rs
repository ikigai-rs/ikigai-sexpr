#![cfg(feature = "full")]

use ikigai_sexpr::{parse, sexpr_to_sparql, sexpr_to_turtle};
use oxigraph::sparql::SparqlEvaluator;
use oxrdfio::{RdfFormat, RdfParser};

#[test]
fn rdf_type_keyword_is_independent_of_rdf_prefix_binding() {
    let input =
        parse(r#"(graph (prefix (rdf "urn:other:")) ((iri "urn:s") a (iri "urn:o")))"#).unwrap();
    let turtle = sexpr_to_turtle(&input).unwrap();
    let triples = RdfParser::from_format(RdfFormat::Turtle)
        .for_slice(turtle.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        triples[0].predicate.as_str(),
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
    );
}

#[test]
fn sparql_rejects_literals_in_subject_and_predicate_positions() {
    for source in [
        r#"(select * (where ("subject" ?p ?o)))"#,
        r#"(select * (where (42 ?p ?o)))"#,
        r#"(select * (where (?s "predicate" ?o)))"#,
        r#"(select * (where (?s 42 ?o)))"#,
    ] {
        let value = parse(source).unwrap();
        assert!(sexpr_to_sparql(&value).is_err(), "accepted {source}");
    }
    let value = parse(r#"(select * (where (?s (iri "urn:p") "object")))"#).unwrap();
    let query = sexpr_to_sparql(&value).unwrap();
    let _ = SparqlEvaluator::new().parse_query(&query).unwrap();
}
