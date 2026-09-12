//! The module recipe as one test: `ikigai-conformance` walks the four endpoints
//! [`ikigai_sexpr::space`] binds and reports every violation at once — plus the
//! by-hand pins for the contracts the suite cannot yet see.
//!
//! ## Declarations, and why each
//!
//! All four endpoints are `pure` AND `cacheable`. Each is a total function of the
//! document TEXT it is handed — the reader, the two compilers and the code-graph
//! codec are the crate's kernel-free layer, and the endpoints resolve nothing:
//! they never touch a file, the network, a clock or the platform, and never issue
//! a sub-request. So an empty golden-thread set on a `.cacheable()` result is
//! right, not a resource nothing can cut. (An endpoint that resolved its input by
//! IRI would be exactly as cacheable as that sub-resolution and would have to be
//! pinned both ways, threaded and live; none here does.)
//!
//! ## Fixtures
//!
//! The suite's minimal scalar is `x`, which is not a valid s-expression, so every
//! action takes a [`Fixture`]. The graph fixture yields two triples over defined
//! terms (a fixture for an RDF face that yields ZERO triples makes SKOLEM-RDF and
//! VOCABULARY pass vacuously), and the code-graph fixture is ENCODED by the crate
//! rather than pasted, so the content-addressed node IRIs can never go stale.
//!
//! ## Namespace
//!
//! `urn:sexpr:to-rdf` serves `sx:root` / `sx:symbol` under
//! `https://ikigai-rs.dev/ns/sexpr#` — a namespace this crate defines and serves
//! ENTIRELY on its own (see [`ikigai_sexpr::SX_NS`]): it is deliberately not part
//! of the shared `ikigai-rs.dev/ns` vocabulary while the encoding is still
//! settling, and nothing under `ik:` is invented for it. Registered here, named in
//! the README.
//!
//! No opt-outs, and NAMES runs: every id is already a kebab-case noun.

use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{ArgRef, Capability, Error, Iri, Kernel, Representation, Request, Verb};
use ikigai_sexpr::{
    parse, sexpr_to_rdf, space, MEDIA_SEXPR, MEDIA_SPARQL_QUERY, MEDIA_TURTLE,
    MEDIA_TURTLE_CODE_GRAPH, SX_NS,
};
use std::sync::Arc;

/// Every endpoint `space()` binds: its IRI, its description id, and the document
/// its fixture feeds it. Order is the binding order.
const ENDPOINTS: [(&str, &str); 4] = [
    ("urn:sparql:from-sexpr", "sparql-from-sexpr"),
    ("urn:rdf:from-sexpr", "rdf-from-sexpr"),
    ("urn:sexpr:to-rdf", "sexpr-to-rdf"),
    ("urn:sexpr:from-rdf", "sexpr-from-rdf"),
];

/// A query valid over any graph: the smallest `(select …)` the compiler accepts.
const QUERY: &str = "(select (?s) (where (?s ?p ?o)) (limit 1))";

/// A graph fixture that yields TWO triples over DEFINED terms — `ik:Endpoint` is
/// in ikigai-vocab, `dcterms:title` is well-known — with a skolemized subject.
/// A fixture yielding zero triples would make both RDF checks vacuous.
const GRAPH: &str = concat!(
    "(graph\n",
    "  (prefix (dcterms \"http://purl.org/dc/terms/\"))\n",
    "  ((iri \"urn:example:conformance\") a (iri \"https://ikigai-rs.dev/ns#Endpoint\"))\n",
    "  ((iri \"urn:example:conformance\") dcterms:title \"conformance\"))"
);

/// A datum exercising all four `Sexpr` cases (symbol, string, integer, nested
/// list), so the code-graph face carries a cons chain and all three atom
/// datatypes rather than a single literal.
const DATUM: &str = "(select (?s) \"conformance\" 1)";

/// The code-graph Turtle `urn:sexpr:from-rdf` decodes — produced by the encoder
/// rather than pasted, so the content-addressed `urn:sexpr:<hash>` node IRIs
/// cannot drift out of date.
fn code_graph() -> String {
    sexpr_to_rdf(&parse(DATUM).expect("DATUM parses")).expect("DATUM encodes")
}

/// The document each action's fixture feeds, by description id.
fn fixture_input(id: &str) -> String {
    match id {
        "sparql-from-sexpr" => QUERY.to_string(),
        "rdf-from-sexpr" => GRAPH.to_string(),
        "sexpr-to-rdf" => DATUM.to_string(),
        "sexpr-from-rdf" => code_graph(),
        other => panic!("no fixture for `{other}`"),
    }
}

/// The suite, configured for this module (see the file docs for why each line).
fn suite() -> Suite {
    ENDPOINTS
        .iter()
        .fold(Suite::new().namespace(SX_NS), |suite, (_, id)| {
            suite
                .fixture(Fixture::new(*id, Verb::Source).arg("content", fixture_input(id)))
                .pure(*id)
                .cacheable(*id)
        })
}

fn kernel() -> Kernel {
    Kernel::new(Arc::new(space()))
}

/// Resolve `iri` under root with the given inline arguments.
fn resolve(args: &[(&str, &str)], iri: &str) -> Result<Representation, Error> {
    let request = args.iter().fold(
        Request::new(Verb::Source, Iri::parse(iri).expect("a valid IRI")),
        |request, (name, value)| request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec())),
    );
    futures::executor::block_on(kernel().issue(request, &Capability::root()))
}

#[test]
fn conforms() {
    let report = suite().run_blocking(&kernel());
    // Printed even when clean (`--nocapture`): the report is the record.
    eprintln!("{report}");
    assert!(report.is_clean(), "{report}");
    // The walk saw exactly the endpoints declared above. A fifth bound without a
    // `pure`/`cacheable` line would be held to a weaker standard (the suite cannot
    // know which endpoints it was not told about); a declared id that binds
    // nothing is a stale list. Both change this count or fail the checks above.
    assert_eq!(report.endpoints, ENDPOINTS.len(), "{report}");
    // Source + Meta is ONE action: `action_specs()` filters Meta.
    assert_eq!(
        report.actions,
        ENDPOINTS.len(),
        "one Source action per endpoint: {report}"
    );
    assert_eq!(
        report.checks.skipped().count(),
        0,
        "every check runs: {report}"
    );
}

/// **OUTPUTS, by hand** (the suite's version landed after 0.1.0): the bare media
/// type each action SERVES with its fixture and no `as=` must be one it DECLARES,
/// and every declared output must be one it can serve. Both directions, because a
/// wrong declaration in either is a face a consumer cannot reach: `outputs` is
/// what the manifold announces, what selection routes on, and what the suite's own
/// RDF checks filter before probing.
///
/// Each endpoint here declares exactly ONE output, so the two directions collapse
/// into one equality per endpoint — stated as a list so a second face added to any
/// of them fails here until it is declared. Note `urn:sexpr:to-rdf` declares
/// `text/turtle` WITH a `profile` parameter (the code-graph marker `urn:transrept:auto`
/// disambiguates on) and serves the same type with the same parameter, so the
/// comparison is on the bare type — parameters are a refinement of a face, not a
/// different one.
#[test]
fn every_action_serves_the_face_it_declares_and_declares_the_face_it_serves() {
    let expected = [
        ("urn:sparql:from-sexpr", MEDIA_SPARQL_QUERY, QUERY),
        ("urn:rdf:from-sexpr", MEDIA_TURTLE, GRAPH),
        ("urn:sexpr:to-rdf", MEDIA_TURTLE_CODE_GRAPH, DATUM),
        ("urn:sexpr:from-rdf", MEDIA_SEXPR, &code_graph()),
    ];
    let kernel = kernel();
    for (iri, face, input) in expected {
        let description = kernel
            .describe_pattern(iri)
            .unwrap_or_else(|| panic!("{iri} describes itself"));
        // Declared: exactly the one face, and it is the one this test names.
        assert_eq!(
            description.outputs,
            vec![face.to_string()],
            "{iri} declares exactly the face this test pins"
        );
        // Served, with the fixture and NO `as=` (an `as` would be the CALLER's
        // label, not the endpoint's own choice of face).
        let served = resolve(&[("content", input)], iri)
            .unwrap_or_else(|e| panic!("{iri} resolves with its fixture: {e}"))
            .repr_type
            .media_type;
        assert_eq!(
            bare(&served),
            bare(face),
            "{iri} serves `{served}` but declares `{face}`"
        );
    }
}

/// The transreptor's declared `outputs` ARE its `transreptsTo` list. The manifold
/// and the RDF checks read `outputs`; `urn:transrept:auto` routes on `transreptsTo`.
/// If the two drift, one consumer sees a face the other cannot reach.
#[test]
fn declared_outputs_are_the_transreption_targets() {
    let kernel = kernel();
    for (iri, _) in ENDPOINTS {
        let description = kernel.describe_pattern(iri).expect("describes itself");
        let transreption = description
            .transreption()
            .unwrap_or_else(|| panic!("{iri} is an ik:Transreptor"));
        assert_eq!(
            description.outputs, transreption.to,
            "{iri}: declared outputs and transreptsTo must be the same list, in order"
        );
    }
}

/// **REQUIRED-IS-REQUIRED, by hand** (conformance PENDING #49/#99): drop each
/// required by-value input from the minimal call and expect a typed refusal; a
/// call that SUCCEEDS without it means "declared required, actually optional".
///
/// This module declares NO required input, deliberately — `content` (the pipe's
/// landing name) and `in` (the named alternative) are two spellings of one
/// document, and `ArgSpec` has no "exactly one of" group. Declaring `content`
/// required was the state before this arc and it was the #49 finding exactly: an
/// `in=`-only call succeeds, so "required" was false, and a SHACL pre-flight over
/// the manifold would have refused a valid call. So the pin runs the other way:
/// each spelling alone WORKS, and neither is a typed `MissingArgument`.
#[test]
fn either_intake_alone_works_and_neither_is_a_typed_missing_argument() {
    for (iri, id) in ENDPOINTS {
        let input = fixture_input(id);
        for name in ["content", "in"] {
            resolve(&[(name, &input)], iri)
                .unwrap_or_else(|e| panic!("{iri} accepts `{name}=` alone: {e}"));
        }
        match resolve(&[], iri) {
            Err(Error::MissingArgument(name)) => assert_eq!(name, "content", "{iri}"),
            other => panic!("{iri} with no document: expected MissingArgument, got {other:?}"),
        }
    }
}

/// **CLASS-IS-ENFORCED, by hand** (conformance PENDING #118/#124): send a
/// non-lexical value to each classed input and expect `InvalidArgument`.
///
/// Every input here is classed `xsd:string` and every UTF-8 byte string is a valid
/// `xsd:string` lexical form, so there is no value that VIOLATES the declared
/// class — the check is vacuous by construction, not skipped, and `xsd:string` is
/// the honest declaration for a text document (the type the wire carries).
///
/// What IS checkable is the layer below: the class is as strong as XSD gets, and
/// the real contract — "a valid s-expression document of the right shape" — is
/// enforced at invoke. So a value that is a fine `xsd:string` and not a valid
/// document must come back as a typed `InvalidArgument` NAMING the input the
/// caller passed, never as a panic, a success, or an untyped `Endpoint` string
/// that an agent would have to sniff to know it was its own input at fault.
/// `(` is unreadable for all four (an unbalanced list, and not Turtle either), so
/// it is the one malformed document every endpoint must reject. `x` — the suite's
/// own minimal scalar, and a one-symbol datum — is rejected by three of them but
/// is a PERFECTLY VALID input to `urn:sexpr:to-rdf`, which is total over every
/// datum the reader produces: its only argument error is a reader error. That
/// asymmetry is the encoder's whole point, so it is pinned rather than papered over.
#[test]
fn a_well_typed_but_malformed_document_is_an_invalid_argument_naming_the_input() {
    let unreadable: Vec<(&str, &str)> = ENDPOINTS.iter().map(|(iri, _)| (*iri, "(")).collect();
    let not_this_endpoints_shape = vec![
        // A bare symbol: a valid datum, not a `(select …)`.
        ("urn:sparql:from-sexpr", "x"),
        // A bare symbol: a valid datum, not a `(graph …)`.
        ("urn:rdf:from-sexpr", "x"),
        // Not Turtle at all.
        ("urn:sexpr:from-rdf", "x"),
    ];
    for (iri, bad) in unreadable.into_iter().chain(not_this_endpoints_shape) {
        for name in ["content", "in"] {
            match resolve(&[(name, bad)], iri) {
                Err(Error::InvalidArgument { name: arg, detail }) => {
                    assert_eq!(arg, name, "{iri} names the input the caller passed");
                    assert!(
                        detail.starts_with(iri),
                        "{iri}: the detail says which stage rejected it: {detail}"
                    );
                }
                other => {
                    panic!("{iri} on `{bad}` via `{name}`: expected InvalidArgument, got {other:?}")
                }
            }
        }
    }
    // The converse, so the list above cannot rot into "everything is rejected":
    // the lossless encoder ACCEPTS the bare symbol the other three refuse.
    resolve(&[("content", "x")], "urn:sexpr:to-rdf")
        .expect("urn:sexpr:to-rdf encodes any datum the reader produces, a lone symbol included");
}

/// **The forced-recomputation witness** (conformance PENDING #64): a
/// non-deterministic result marked `.cacheable()` passes CACHEABLE, because the
/// probe's second resolution IS the cache hit — "byte-identical" is tautological
/// and the suite never sees two COMPUTATIONS. #64's suggested fix (cut every
/// thread and resolve again) cannot apply to a `pure` endpoint: there is no thread
/// to cut. A second KERNEL is the witness that works here — a fresh cache over the
/// same space, so the bytes come from a genuinely independent computation.
///
/// This matters concretely for `urn:sexpr:to-rdf`: its node IRIs are content
/// hashes and its statements are sorted, so the output is byte-stable — but a
/// future encoder that emitted prefixes or statements in `HashMap` order would be
/// non-deterministic under `RandomState` and every consumer would cache the first
/// answer forever. That is the bug this test exists to fail on.
#[test]
fn every_cacheable_result_is_the_same_bytes_from_an_independent_computation() {
    for (iri, id) in ENDPOINTS {
        let input = fixture_input(id);
        // `resolve` builds a FRESH kernel per call, so the second resolution runs
        // over a cold cache: a second computation, not the first one's cached answer.
        let once = resolve(&[("content", &input)], iri).expect("resolves");
        let twice = resolve(&[("content", &input)], iri).expect("resolves");
        assert_eq!(
            once.bytes, twice.bytes,
            "{iri} is marked cacheable but two independent computations differ: not a \
             function of its inputs"
        );
        assert_eq!(
            once.repr_type.media_type, twice.repr_type.media_type,
            "{iri}"
        );
    }
}

/// The media type without its parameters — what a declared output and a served
/// face are compared on (`text/turtle; profile="…"` is the `text/turtle` face).
fn bare(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}
