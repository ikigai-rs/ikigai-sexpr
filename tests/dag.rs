//! A code-graph that SHARES its nodes (a DAG) is refused once what it decodes to passes the
//! crate's size bounds, quickly and without first building the tree (ledger #1331).
//!
//! The claim: `rdf_to_sexpr` expands a shared node at every place it is used, and nothing
//! counted what it produced, so cons nodes shared as a DAG (`a_i first=a_{i+1} rest=b_i`,
//! `b_i first=a_{i+1} rest=nil`) decode to a tree that doubles with every level. Measured
//! against the code before the fix, in a debug build, through `urn:sexpr:from-rdf`: 16 levels
//! (5.7 KB of Turtle) answered 393 KB in 0.2 s, 20 levels (7.0 KB) 6.3 MB in 2.7 s, 22 levels
//! (7.7 KB) 25 MB in 11 s, so about 40 levels (~14 KB) exhausts a host's memory: one request
//! kills the server, the same class as ledger #1326, by memory instead of by stack.
//!
//! The probes run in a CHILD PROCESS (this test binary re-executed with one probe named in its
//! environment) so a probe that expands can be killed at a deadline and read here as a
//! failure, instead of taking the test run's memory with it.
#![cfg(feature = "full")]

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Verb};
use ikigai_sexpr::{parse, rdf_to_sexpr, sexpr_to_rdf, write, Sexpr};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROBE: &str = "IKIGAI_SEXPR_DAG_PROBE";
/// The bounds, restated so these tests read the same against the code that predates them.
const NODES: usize = 1 << 20;
const TEXT: usize = 32 << 20;
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const SX: &str = "https://ikigai-rs.dev/ns/sexpr#";
/// Generous for a debug build on a loaded CI runner: the refusal itself takes well under a
/// second, and the expansion it replaces took 11 s at 22 levels and doubles per level.
const DEADLINE: Duration = Duration::from_secs(30);

/// The ledger's shape, `levels` deep: `a_i = (a_{i+1} . b_i)` and `b_i = (a_{i+1})`, so `a_i`
/// is the two-element list `(a_{i+1} a_{i+1})` sharing ONE node for both elements. The leaf
/// `a_levels` is `(leaf)`. The Turtle grows by two statements per level; the tree it denotes
/// doubles.
fn dag(levels: usize, leaf: &str) -> String {
    let mut t = format!("<urn:sexpr:document> <{SX}root> <urn:sexpr:a0> .\n");
    for i in 0..levels {
        let next = i + 1;
        t += &format!(
            "<urn:sexpr:a{i}> <{RDF_FIRST}> <urn:sexpr:a{next}> ; <{RDF_REST}> <urn:sexpr:b{i}> .\n\
             <urn:sexpr:b{i}> <{RDF_FIRST}> <urn:sexpr:a{next}> ; <{RDF_REST}> <{RDF_NIL}> .\n"
        );
    }
    t + &format!("<urn:sexpr:a{levels}> <{RDF_FIRST}> {leaf} ; <{RDF_REST}> <{RDF_NIL}> .\n")
}

/// A symbol literal for `dag`'s leaf.
fn symbol(s: &str) -> String {
    format!("\"{s}\"^^<{SX}symbol>")
}

/// How many nodes `dag(levels, _)` decodes to: the leaf list holds 2 (itself and its atom),
/// and each level is one list holding two copies of the level below.
fn dag_nodes(levels: u32) -> usize {
    (0..levels).fold(2, |n, _| 1 + 2 * n)
}

fn from_rdf(doc: String) -> ikigai_core::Result<String> {
    let kernel = Kernel::new(Arc::new(ikigai_sexpr::space()));
    let request = Request::new(
        Verb::Source,
        Iri::parse("urn:sexpr:from-rdf".to_string()).unwrap(),
    )
    .with_arg("content", ArgRef::Inline(doc.into_bytes()));
    block_on(kernel.issue(request, &Capability::root()))
        .map(|rep| String::from_utf8_lossy(&rep.bytes).into_owned())
}

/// The child's half: inert unless a parent named a probe (a level count). Decodes the DAG
/// through the door on a 2 MiB thread (a tokio worker's), prints the outcome, and exits
/// before the harness can.
#[test]
fn probe_child() {
    let Ok(levels) = std::env::var(PROBE) else {
        return;
    };
    let levels = levels.parse::<usize>().unwrap();
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || match from_rdf(dag(levels, &symbol("x"))) {
            Ok(text) => format!("ok {} bytes", text.len()),
            Err(e) => format!("err {}", e.to_string().replace('\n', " ")),
        })
        .unwrap()
        .join()
        .unwrap();
    println!("\nOUTCOME {outcome}");
    std::process::exit(0);
}

/// The parent's half: run one probe in a child, killing it at [`DEADLINE`], and return what
/// it said and how long it took. A probe still running at the deadline is the defect.
fn probe(levels: usize) -> (String, Duration) {
    let started = Instant::now();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, levels.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > DEADLINE {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "a {levels}-level DAG code-graph was still decoding after {DEADLINE:?}: it \
                 expanded instead of being refused"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let elapsed = started.elapsed();
    // Reaped above; `wait_with_output` now only drains the pipes. (Small output: the child
    // prints one line, so the pipes cannot fill and stall it before it exits.)
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the {levels}-level DAG probe died: {} — {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let at = stdout
        .find("\nOUTCOME ")
        .unwrap_or_else(|| panic!("the {levels}-level DAG probe reported nothing: {stdout}"));
    let line = stdout[at + "\nOUTCOME ".len()..]
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    (line, elapsed)
}

#[test]
fn a_shared_code_graph_is_refused_quickly_instead_of_expanding() {
    // 24 levels is ~5 KB of Turtle denoting a tree of 50 million nodes; 64 would be ~10^20.
    for levels in [24, 64] {
        let (outcome, took) = probe(levels);
        assert!(
            outcome.starts_with("err invalid argument `content`: urn:sexpr:from-rdf: ")
                && outcome.contains("more than 1048576 s-expression nodes"),
            "{levels} levels: {outcome}"
        );
        assert!(took < DEADLINE, "{levels} levels took {took:?}");
    }
}

#[test]
fn a_shared_code_graph_within_the_bounds_still_decodes() {
    // Sharing is legitimate (the encoder emits it for every repeated sub-list): a DAG whose
    // tree fits the bounds decodes, and comes back as the tree.
    let levels = 10;
    let decoded = rdf_to_sexpr(&dag(levels, &symbol("x"))).unwrap();
    let mut expect = Sexpr::List(vec![Sexpr::Symbol("x".to_string())]);
    for _ in 0..levels {
        expect = Sexpr::List(vec![expect.clone(), expect]);
    }
    assert_eq!(decoded, expect);
}

#[test]
fn the_node_bound_is_exact_and_names_itself() {
    // 18 levels decode to 786,431 nodes and 19 to 1,572,863: under and over the bound.
    assert!(dag_nodes(18) <= NODES && dag_nodes(19) > NODES);
    assert!(rdf_to_sexpr(&dag(18, &symbol("x"))).is_ok());
    let refused = rdf_to_sexpr(&dag(19, &symbol("x")))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("more than 1048576 s-expression nodes"),
        "{refused}"
    );
}

#[test]
fn the_text_bound_counts_a_shared_literal_at_every_place_it_is_used() {
    // One 1 MiB literal, used 2^6 = 64 times: 64 MiB of text from ~1 MiB of Turtle, and only
    // 191 nodes, so the node bound alone would let it through.
    let big = format!("\"{}\"^^<{SX}symbol>", "x".repeat(1 << 20));
    assert!(dag_nodes(6) < 200);
    let refused = rdf_to_sexpr(&dag(6, &big)).unwrap_err().to_string();
    assert!(
        refused.contains("more than 33554432 bytes of s-expression text"),
        "{refused}"
    );
    // 2^5 = 32 uses is exactly the bound, and decodes.
    assert!(rdf_to_sexpr(&dag(5, &big)).is_ok());
}

/// `(d d)` nested `levels` times around `leaf`: the reader reads it from text that grows with
/// the tree, and the encoder writes it as a DAG that grows with `levels`.
fn doubled(levels: usize, leaf: Sexpr) -> Sexpr {
    (0..levels).fold(leaf, |d, _| Sexpr::List(vec![d.clone(), d]))
}

/// The codec stays closed at both bounds: a datum the reader accepts AT a bound encodes and
/// decodes back unchanged, and one past it is refused by the reader and the decoder alike.
#[test]
fn a_datum_the_reader_accepts_at_the_bounds_round_trips_and_one_past_is_refused_by_both() {
    // Nodes: `(d)` with d = 19 doublings of a symbol is 1 + (2^20 - 1) = 2^20 nodes, exactly.
    let at = Sexpr::List(vec![doubled(19, Sexpr::Symbol("x".to_string()))]);
    // One more node: a second atom in the outer list.
    let past = Sexpr::List(vec![
        doubled(19, Sexpr::Symbol("x".to_string())),
        Sexpr::Int(0),
    ]);
    // Text: 16 copies of a 2 MiB string is 32 MiB exactly; one more byte is past it.
    let mib = Sexpr::Str("y".repeat(TEXT / 16));
    let text_at = doubled(4, mib.clone());
    let text_past = Sexpr::List(vec![doubled(4, mib), Sexpr::Symbol("z".to_string())]);

    for (what, datum) in [("nodes", at), ("text", text_at)] {
        let read = parse(&write(&datum)).unwrap_or_else(|e| panic!("{what} at the bound: {e}"));
        assert_eq!(
            read, datum,
            "{what}: the reader reads what the printer wrote"
        );
        let graph = sexpr_to_rdf(&read).unwrap();
        let back = rdf_to_sexpr(&graph).unwrap_or_else(|e| panic!("{what} at the bound: {e}"));
        assert!(back == datum, "{what} at the bound round-trips");
    }
    for (what, datum, words) in [
        ("nodes", past, "more than 1048576 s-expression nodes"),
        (
            "text",
            text_past,
            "more than 33554432 bytes of s-expression text",
        ),
    ] {
        let read = parse(&write(&datum)).unwrap_err().to_string();
        assert!(read.contains(words), "{what} past, the reader: {read}");
        let graph = sexpr_to_rdf(&datum).unwrap();
        let decoded = rdf_to_sexpr(&graph).unwrap_err().to_string();
        assert!(
            decoded.contains(words),
            "{what} past, the decoder: {decoded}"
        );
    }
}
