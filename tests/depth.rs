//! Caller input nested deeply is refused before anything recurses over it, at every door that
//! parses request input, and a document at the bound is answered on a small thread (ledger
//! #1326).
//!
//! The claim: a door serves a request on a tokio worker with the default 2 MiB stack, and a
//! stack overflow aborts the WHOLE process, on any thread, so one deep request can kill a host.
//! A size bound does not bound depth. Measured against the code before the fix, in a debug
//! build: `urn:sexpr:from-rdf` ABORTED (SIGABRT, "has overflowed its stack") on a code-graph
//! nested 1,800 lists deep (~100 KB of Turtle), and on every depth tried past it, because its
//! decoder allowed 4,096 levels while the stack held fewer than 1,800. 1,500 survived. Every
//! other door already refused or never recursed: the s-expression reader stops at 512 lists,
//! core's `Topology::from_turtle` at 48 spaces, and oxttl parses Turtle collections and blank
//! node property lists 100,000 deep without recursing.
//!
//! Every probe runs in a CHILD PROCESS (this test binary re-executed with one probe named in
//! its environment) on a 2 MiB thread, the size of a tokio worker's, so an abort kills the
//! child and reads here as a failure with the signal named.
#![cfg(feature = "full")]

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Verb};
use std::process::Command;
use std::sync::Arc;

const PROBE: &str = "IKIGAI_SEXPR_DEPTH_PROBE";
/// The bound, restated so these tests read the same against the code that predates it.
const BOUND: usize = 512;
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

/// The doors that read an s-expression from the caller.
const SEXPR_DOORS: [&str; 4] = [
    "urn:sparql:from-sexpr",
    "urn:rdf:from-sexpr",
    "urn:sexpr:to-rdf",
    "urn:sexpr:arrangement-to-rdf",
];
/// The doors that read Turtle from the caller.
const TURTLE_DOORS: [&str; 2] = ["urn:sexpr:from-rdf", "urn:sexpr:arrangement-from-rdf"];

/// A document of shape `name`, `n` levels deep.
fn shape(name: &str, n: usize) -> String {
    let r = |s: &str, k: usize| s.repeat(k);
    match name {
        // Bare lists: `n` lists.
        "lists" => format!("{}x{}", r("(", n), r(")", n)),
        // Deep where each compiler reads a term or an object.
        "select" => format!("(select * (where (?s ?p {}x{})))", r("(", n), r(")", n)),
        "graph" => format!(
            "(graph (prefix (ex \"urn:ex:\")) (ex:s ex:p {}x{}))",
            r("(", n),
            r(")", n)
        ),
        // An arrangement's own nesting: `n` fallbacks around a leaf.
        "fallbacks" => format!("{}(limit \"urn:x:\"){}", r("(fallback ", n), r(")", n)),
        // A code-graph of `n + 1` lists, each the only element of the one around it.
        "code-graph" => {
            let mut t =
                "<urn:sexpr:document> <https://ikigai-rs.dev/ns/sexpr#root> <urn:sexpr:n0> .\n"
                    .to_string();
            for i in 0..n {
                t += &format!(
                    "<urn:sexpr:n{i}> <{RDF_FIRST}> <urn:sexpr:n{}> ; <{RDF_REST}> <{RDF_NIL}> .\n",
                    i + 1
                );
            }
            t + &format!(
                "<urn:sexpr:n{n}> <{RDF_FIRST}> \"x\"^^<https://ikigai-rs.dev/ns/sexpr#symbol> ; \
                 <{RDF_REST}> <{RDF_NIL}> .\n"
            )
        }
        // Turtle's own nesting syntaxes, for the parser: collections and blank node lists.
        "collections" => format!("<urn:ex:s> <urn:ex:p> {}\"x\"{} .", r("( ", n), r(" )", n)),
        "bnodes" => format!(
            "<urn:ex:s> <urn:ex:p> {}\"x\"{} .",
            r("[ <urn:ex:p> ", n),
            r(" ]", n)
        ),
        // A declaration of `n` mounts, each over the next (core's own doctest shape).
        "mounts" => {
            let mut t = String::from("@prefix ik: <https://ikigai-rs.dev/ns#> .\n");
            for i in 1..n {
                t += &format!(
                    "<urn:x:{i}> a ik:Mount ; ik:prefix \"urn:\" ; ik:space <urn:x:{}> .\n",
                    i + 1
                );
            }
            t + &format!("<urn:x:{n}> a ik:EndpointSpace ; ik:doors <{RDF_NIL}> .\n")
        }
        other => panic!("no shape named {other}"),
    }
}

fn issue(door: &str, doc: String) -> ikigai_core::Result<String> {
    let kernel = Kernel::new(Arc::new(ikigai_sexpr::space()));
    let request = Request::new(Verb::Source, Iri::parse(door.to_string()).unwrap())
        .with_arg("content", ArgRef::Inline(doc.into_bytes()));
    block_on(kernel.issue(request, &Capability::root()))
        .map(|rep| String::from_utf8_lossy(&rep.bytes).into_owned())
}

/// What a probe does: one door, or `round-trip` (`urn:sexpr:to-rdf` then `urn:sexpr:from-rdf`,
/// answering whether the datum came back unchanged).
fn run(door: &str, doc: String) -> ikigai_core::Result<String> {
    if door == "round-trip" {
        let graph = issue("urn:sexpr:to-rdf", doc.clone())?;
        let back = issue("urn:sexpr:from-rdf", graph)?;
        return Ok(format!("same={}", back == doc));
    }
    issue(door, doc)
}

/// The child's half: inert unless a parent named a probe. Runs it on a 2 MiB thread, prints
/// the outcome, and exits before the harness can.
#[test]
fn probe_child() {
    let Ok(spec) = std::env::var(PROBE) else {
        return;
    };
    let mut parts = spec.rsplitn(3, '|');
    let n = parts.next().unwrap().parse::<usize>().unwrap();
    let name = parts.next().unwrap().to_string();
    let door = parts.next().unwrap().to_string();
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || match run(&door, shape(&name, n)) {
            Ok(text) => format!("ok {}", text.chars().take(200).collect::<String>()),
            Err(e) => format!("err {}", e.to_string().replace('\n', " ")),
        })
        .unwrap()
        .join()
        .unwrap();
    println!("\nOUTCOME {}", outcome.replace('\n', " "));
    std::process::exit(0);
}

/// The parent's half: run one probe in a child and return what it said, or panic naming how it
/// died. An abort is the defect.
fn probe(door: &str, name: &str, n: usize) -> String {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, format!("{door}|{name}|{n}"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{name} nested {n} deep through `{door}` did not survive a 2 MiB thread: {} — {}",
        out.status,
        stderr
            .lines()
            .find(|l| l.contains("overflow"))
            .unwrap_or(&stderr)
    );
    let at = stdout
        .find("\nOUTCOME ")
        .unwrap_or_else(|| panic!("the `{door}` {name} probe reported nothing: {stdout}"));
    stdout[at + "\nOUTCOME ".len()..]
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

fn assert_refused(outcome: &str, door: &str, depth_words: &str, what: &str) {
    assert!(
        outcome.starts_with(&format!("err invalid argument `content`: {door}: "))
            && outcome.contains(depth_words),
        "{what}: {outcome}"
    );
}

#[test]
fn deep_s_expressions_are_refused_at_every_door_and_abort_nothing() {
    for door in SEXPR_DOORS {
        for name in ["lists", "select", "graph", "fallbacks"] {
            for n in [1500, 100_000] {
                assert_refused(
                    &probe(door, name, n),
                    door,
                    "nests deeper than the reader limit (512)",
                    &format!("{door} {name} {n}"),
                );
            }
        }
    }
}

#[test]
fn a_deep_code_graph_is_refused_and_aborts_nothing() {
    // 1,800 is the depth that aborted before the fix; 100,000 is far past any stack.
    for n in [1800, 100_000, 4096, BOUND] {
        assert_refused(
            &probe("urn:sexpr:from-rdf", "code-graph", n),
            "urn:sexpr:from-rdf",
            "code-graph nests deeper than 512 lists",
            &format!("code-graph {n}"),
        );
    }
}

#[test]
fn deep_turtle_syntax_and_deep_declarations_abort_nothing() {
    // oxttl does not recurse on Turtle's nesting syntaxes: each document parses, and is then
    // refused for what it is not (a code-graph, a declaration), never for its depth.
    for door in TURTLE_DOORS {
        for name in ["collections", "bnodes"] {
            let outcome = probe(door, name, 100_000);
            assert!(outcome.starts_with("err "), "{door} {name}: {outcome}");
            assert!(
                !outcome.contains("nests deeper"),
                "{door} {name}: {outcome}"
            );
        }
    }
    // A declaration is core's to bound, and core refuses it at 48 spaces.
    assert_refused(
        &probe("urn:sexpr:arrangement-from-rdf", "mounts", 100_000),
        "urn:sexpr:arrangement-from-rdf",
        "nests deeper than 48 spaces (MAX_DECLARATION_DEPTH)",
        "mounts 100000",
    );
}

/// At the bound every door answers, on the 2 MiB thread the probe gives it, through every walk
/// that follows the read (the encoder, the decoder, the printer, `Drop`); one past is refused.
#[test]
fn a_datum_at_the_bound_is_answered_on_a_small_thread_and_one_past_is_refused() {
    let ok = probe("urn:sexpr:to-rdf", "lists", BOUND);
    assert!(ok.starts_with("ok @prefix"), "to-rdf at the bound: {ok}");
    assert_refused(
        &probe("urn:sexpr:to-rdf", "lists", BOUND + 1),
        "urn:sexpr:to-rdf",
        "nests deeper than the reader limit (512)",
        "to-rdf one past",
    );

    // `code-graph` nests `n + 1` lists.
    let ok = probe("urn:sexpr:from-rdf", "code-graph", BOUND - 1);
    assert_eq!(
        ok,
        format!("ok {}", "(".repeat(200)),
        "from-rdf at the bound"
    );
    assert_refused(
        &probe("urn:sexpr:from-rdf", "code-graph", BOUND),
        "urn:sexpr:from-rdf",
        "code-graph nests deeper than 512 lists",
        "from-rdf one past",
    );

    // The codec is closed at the bound: the deepest datum the reader yields comes back whole.
    assert_eq!(probe("round-trip", "lists", BOUND), "ok same=true");
}
