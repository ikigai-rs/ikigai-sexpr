//! **The s-expression endpoints** — `urn:sparql:from-sexpr`, `urn:rdf:from-sexpr` and the
//! code-graph pair `urn:sexpr:to-rdf` / `urn:sexpr:from-rdf`. Behind the `full` feature, with
//! [`space`](crate::space), which mounts them beside the arrangement pair.

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, Error as CoreError, Invocation, ReprType, Representation,
    Result as CoreResult, Verb,
};

use crate::{
    content_summary, in_summary, parse, rdf_to_sexpr, read_source, sexpr_to_rdf, sexpr_to_sparql,
    sexpr_to_turtle, write, SexprError, MEDIA_SEXPR, MEDIA_SPARQL_QUERY, MEDIA_TURTLE,
    MEDIA_TURTLE_CODE_GRAPH, XSD_STRING,
};

/// The distinguishing output profile of the `urn:sexpr:to-rdf` code-graph. Same base media
/// type as 3c's domain Turtle (`text/turtle`) but a distinct profile, so `urn:transrept:auto`
/// can tell the lossless-structure transreption apart from the domain-graph one.
const CODE_GRAPH_PROFILE: &str = "https://ikigai-rs.dev/ns/sexpr#code-graph";

/// The `urn:sparql:from-sexpr` transreptor: read an s-expr query TEXT (piped `content`, or
/// a named `in`), [`parse`] it, [`sexpr_to_sparql`] it, and emit the SPARQL string. A
/// first-class `ik:Transreptor` (`text/x-sexpr` → `application/sparql-query`) — no lisp
/// engine involved. Pure function of its input bytes, so its result is `.cacheable()`
/// (the kernel folds in the piped source's expiry down the pipe).
pub(crate) struct FromSexpr;

#[async_trait]
impl Endpoint for FromSexpr {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let (src, arg) = read_source(inv)?;
        let sexpr = parse(src).map_err(|e| invalid(arg, "urn:sparql:from-sexpr", &e))?;
        let sparql =
            sexpr_to_sparql(&sexpr).map_err(|e| invalid(arg, "urn:sparql:from-sexpr", &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_SPARQL_QUERY).with_param("charset", "utf-8"),
            sparql.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sparql-from-sexpr"
    }

    fn describe(&self) -> Description {
        Description::new("sparql-from-sexpr")
            .title("SPARQL from s-expression")
            .summary(
                "Compile an s-expression SELECT into a SPARQL query — a language-agnostic \
                 transreptor with no lisp engine. Pipe an s-expr query in (or pass `in=`); the \
                 form is `(select (?vars…)|* (where (S P O)…) (prefix (pfx \"…\")…) (order-by …) \
                 (limit N))`. String literals and IRIs are escaped/validated, never \
                 interpolated; a malformed query is a clean error. Output is \
                 application/sparql-query — feed it to urn:sparql:select as `query=`.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the s-expression query TEXT to compile"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the s-expression query TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_SPARQL_QUERY)
            // First-class `ik:Transreptor`: an s-expr query document → a SPARQL query.
            // LOSSY (ledger #645): `text/x-sexpr` names an s-expression DATUM, and this
            // compiles the datum under one profile — `(select …)` only, clauses reordered to
            // canonical order, `pfx` and `pfx:` alike — so distinct datums give the same query
            // and nothing maps the query back. A lossless-only planner must not route a
            // `text/x-sexpr` document through it; a caller invokes it by name, or consents.
            .transreptor([MEDIA_SEXPR], [MEDIA_SPARQL_QUERY])
            .lossy()
    }
}

/// The `urn:rdf:from-sexpr` transreptor: read an s-expr graph TEXT (piped `content`, or a
/// named `in`), [`parse`] it, [`sexpr_to_turtle`] it, and emit the Turtle. A first-class
/// `ik:Transreptor` (`text/x-sexpr` → `text/turtle`) — no lisp engine involved. Shares the
/// `text/x-sexpr` input with [`FromSexpr`]; the two disambiguate by output media type and by
/// the document head (`graph` here, `select` there), so each errors on the wrong form. Pure
/// function of its input bytes, so its result is `.cacheable()`.
pub(crate) struct FromSexprTurtle;

#[async_trait]
impl Endpoint for FromSexprTurtle {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let (src, arg) = read_source(inv)?;
        let sexpr = parse(src).map_err(|e| invalid(arg, "urn:rdf:from-sexpr", &e))?;
        let turtle = sexpr_to_turtle(&sexpr).map_err(|e| invalid(arg, "urn:rdf:from-sexpr", &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_TURTLE).with_param("charset", "utf-8"),
            turtle.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "rdf-from-sexpr"
    }

    fn describe(&self) -> Description {
        Description::new("rdf-from-sexpr")
            .title("RDF (Turtle) from s-expression")
            .summary(
                "Compile an s-expression graph into RDF Turtle — a language-agnostic \
                 transreptor with no lisp engine. Pipe an s-expr graph in (or pass `in=`); the \
                 form is `(graph (prefix (pfx \"…\")…) (S P O)…)` where S/P are pfx:local or \
                 (iri \"…\") and O adds \"string\"/integer/(lit \"v\" dt) literals. `a` renders \
                 as rdf:type (rdf: auto-bound); NO blank nodes (skolemize). String literals and \
                 IRIs are escaped/validated, never interpolated; a malformed graph is a clean \
                 error. Output is text/turtle — feed it to urn:rdf:* to convert or store.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the s-expression graph TEXT to compile"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the s-expression graph TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_TURTLE)
            // First-class `ik:Transreptor`: an s-expr graph document → RDF Turtle.
            // LOSSY (ledger #645): it INTERPRETS a `(graph …)` datum as the triples it
            // denotes — a set, so order and repeats are gone, and `42` and
            // `(lit "42" xsd:integer)` are one term — rather than encoding the datum, which is
            // `urn:sexpr:to-rdf`'s job. Declared lossless (the default since core 0.1.77), a
            // lossless-only planner asked for `text/x-sexpr → text/turtle` picked it, refused
            // every s-expression that is not a `(graph …)`, and had no way back.
            .transreptor([MEDIA_SEXPR], [MEDIA_TURTLE])
            .lossy()
    }
}

/// The `urn:sexpr:to-rdf` transreptor: read an s-expr document TEXT (piped `content`, or a
/// named `in`), [`parse`] it, and [`sexpr_to_rdf`] it into the **lossless** code-graph
/// Turtle. A first-class `ik:Transreptor` (`text/x-sexpr` → `text/turtle` with the
/// **code-graph profile**). It shares `text/x-sexpr → text/turtle` with 3c's
/// `urn:rdf:from-sexpr`, but that endpoint INTERPRETS a `(graph …)` form as domain triples
/// while this one encodes the sexpr's STRUCTURE — the distinct output *profile* keeps the two
/// unambiguous for `urn:transrept:auto` (a request for plain domain Turtle selects
/// `urn:rdf:from-sexpr`; the code-graph is opt-in via the profile or explicit invocation).
/// Pure function of its input bytes, so `.cacheable()`.
pub(crate) struct ToRdf;

#[async_trait]
impl Endpoint for ToRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let (src, arg) = read_source(inv)?;
        let sexpr = parse(src).map_err(|e| invalid(arg, "urn:sexpr:to-rdf", &e))?;
        let turtle = sexpr_to_rdf(&sexpr).map_err(|e| invalid(arg, "urn:sexpr:to-rdf", &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_TURTLE)
                .with_param("charset", "utf-8")
                .with_param("profile", CODE_GRAPH_PROFILE),
            turtle.into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-to-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-to-rdf")
            .title("Lossless RDF code-graph from an s-expression")
            .summary(
                "Encode ANY s-expression LOSSLESSLY as an RDF graph — its structure, so code \
                 becomes queryable/signable/routable. Pipe an s-expr in (or pass `in=`): a \
                 list → an rdf:List whose cons-cell IRIs are `urn:sexpr:<hash>` (content-\
                 addressed, so equal sub-lists share a node); atoms → typed literals \
                 (^^sx:symbol / ^^xsd:string / ^^xsd:integer); `<urn:sexpr:document> sx:root` \
                 names the top. Deterministic (byte-stable) and skolemized (no blank nodes). \
                 Output is Turtle with the code-graph profile; reverse with urn:sexpr:from-rdf. \
                 Distinct from urn:rdf:from-sexpr, which reads a (graph …) as domain triples.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the s-expression document TEXT to encode"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the s-expression document TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_TURTLE_CODE_GRAPH)
            // First-class `ik:Transreptor`: an s-expr document → its lossless code-graph.
            // LOSSLESS (the default): every datum encodes to one graph and decodes back
            // exactly (`rdf_to_sexpr(sexpr_to_rdf(x)) == x`); only comments and layout, which
            // are not part of the datum, are not kept.
            .transreptor([MEDIA_SEXPR], [MEDIA_TURTLE_CODE_GRAPH])
    }
}

/// The `urn:sexpr:from-rdf` transreptor: read a code-graph Turtle document (piped `content`,
/// or a named `in`), [`rdf_to_sexpr`] it, and [`write`] the reconstructed s-expression back
/// out. A first-class `ik:Transreptor` (`text/turtle` → `text/x-sexpr`) — the exact inverse
/// of [`ToRdf`] on a code-graph, and declared LOSSY because its input is any Turtle: it
/// extracts the code-graph and skips every other triple (a code-graph embedded in a larger
/// graph decodes), so two different graphs give one s-expression.
/// Pure function of its input bytes, so `.cacheable()`.
pub(crate) struct FromRdf;

#[async_trait]
impl Endpoint for FromRdf {
    async fn invoke(&self, inv: &Invocation<'_>) -> CoreResult<Representation> {
        let (src, arg) = read_source(inv)?;
        let sexpr = rdf_to_sexpr(src).map_err(|e| invalid(arg, "urn:sexpr:from-rdf", &e))?;
        Ok(Representation::new(
            ReprType::new(MEDIA_SEXPR).with_param("charset", "utf-8"),
            write(&sexpr).into_bytes(),
        )
        .cacheable())
    }

    fn name(&self) -> &str {
        "sexpr-from-rdf"
    }

    fn describe(&self) -> Description {
        Description::new("sexpr-from-rdf")
            .title("S-expression from a lossless RDF code-graph")
            .summary(
                "Reconstruct the EXACT s-expression from a lossless code-graph (the inverse of \
                 urn:sexpr:to-rdf). Pipe the Turtle in (or pass `in=`): reads \
                 `<urn:sexpr:document> sx:root`, walks the rdf:List, decodes atoms by datatype \
                 (sx:symbol→symbol, xsd:string→string, xsd:integer→integer). Malformed or \
                 ill-typed input is a clean error. Output is text/x-sexpr.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary(content_summary("the code-graph Turtle TEXT to decode"))
                    .class(XSD_STRING),
            )
            .input(
                ArgSpec::new("in")
                    .summary(in_summary("the code-graph Turtle TEXT"))
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(MEDIA_SEXPR)
            // First-class `ik:Transreptor`: a code-graph → its s-expression.
            // LOSSY (ledger #645, checked while there): the input type is plain `text/turtle`,
            // any graph, and the decoder skips every triple that is not part of the code-graph
            // (pinned by `other_triples_in_the_document_are_ignored` in
            // src/tests.rs) — an EXTRACTION, which core's `Description::lossy` names. Exact on
            // the code-graph itself; lossless would need it to refuse an unrelated triple, as
            // core's `Topology::from_turtle` does for the arrangement pair.
            .transreptor([MEDIA_TURTLE], [MEDIA_SEXPR])
            .lossy()
    }
}

/// A malformed s-expression (or code-graph) document is the ARGUMENT's problem, not the
/// endpoint's. A typed `InvalidArgument` naming the input the caller passed lets a caller —
/// an agent retry loop, the wire's typed errors — tell "fix your input" from "the endpoint
/// broke" without sniffing message text. `iri` prefixes the detail so a pipeline's error
/// still says which stage rejected it.
fn invalid(arg: &str, iri: &str, err: &SexprError) -> CoreError {
    CoreError::InvalidArgument {
        name: arg.to_string(),
        detail: format!("{iri}: {}", err.detail()),
    }
}
