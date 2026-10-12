//! The code-graph encoder costs time in proportion to the datum it encodes, not to the datum
//! times its nesting depth (ledger #1332).
//!
//! The claim: `sexpr_to_rdf` (behind `urn:sexpr:to-rdf`) recomputed the content hash of a
//! list's whole subtree at every level that list is nested in, with no memo, so its cost was
//! nodes x depth. Measured against the code before the fix, in a debug build: a 4,096-atom list
//! wrapped in `d` lists (~8-9 KB of text) took 48 ms at d = 1 and 8.9 s at d = 511, the deepest
//! the reader accepts, so a 2 MB input at that depth is minutes of CPU from one request: the
//! same one-request class as ledger #1326, by CPU instead of by stack.
//!
//! Each probe runs in a CHILD PROCESS (this test binary re-executed with one probe named in
//! its environment) so a probe that runs away can be killed at a deadline and read here as a
//! failure. The child times the request itself, so process start-up is not counted.
#![cfg(feature = "full")]

use futures::executor::block_on;
use ikigai_core::{ArgRef, Capability, Iri, Kernel, Request, Verb};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROBE: &str = "IKIGAI_SEXPR_ENCODER_PROBE";
/// The atoms in the innermost list.
const ATOMS: usize = 4096;
/// The deepest wrapping the reader accepts: 511 lists around the atom list is 512 lists, the
/// reader's bound.
const DEEP: usize = 511;
/// Generous for a debug build on a loaded CI runner: the encode at the deepest wrapping takes
/// tens of milliseconds once each node is hashed once, and took 8.9 s when every level rehashed
/// its subtree.
const CEILING: Duration = Duration::from_secs(3);
/// Past this the child is killed: the probe ran away.
const DEADLINE: Duration = Duration::from_secs(60);

/// `ATOMS` distinct symbols in one list, wrapped in `depth` single-element lists.
fn wrapped(depth: usize) -> String {
    let atoms: Vec<String> = (0..ATOMS).map(|i| format!("a{i}")).collect();
    format!(
        "{}({}){}",
        "(".repeat(depth),
        atoms.join(" "),
        ")".repeat(depth)
    )
}

fn to_rdf(doc: String) -> ikigai_core::Result<String> {
    let kernel = Kernel::new(Arc::new(ikigai_sexpr::space()));
    let request = Request::new(
        Verb::Source,
        Iri::parse("urn:sexpr:to-rdf".to_string()).unwrap(),
    )
    .with_arg("content", ArgRef::Inline(doc.into_bytes()));
    block_on(kernel.issue(request, &Capability::root()))
        .map(|rep| String::from_utf8_lossy(&rep.bytes).into_owned())
}

/// The child's half: inert unless a parent named a probe (a wrapping depth). Encodes through
/// the door on a 2 MiB thread (a tokio worker's), prints the outcome and how long the request
/// took, and exits before the harness can.
#[test]
fn probe_child() {
    let Ok(depth) = std::env::var(PROBE) else {
        return;
    };
    let depth = depth.parse::<usize>().unwrap();
    let doc = wrapped(depth);
    let outcome = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let started = Instant::now();
            let result = to_rdf(doc);
            let micros = started.elapsed().as_micros();
            match result {
                Ok(text) => format!("ok {micros} {}", text.len()),
                Err(e) => format!("err {micros} {}", e.to_string().replace('\n', " ")),
            }
        })
        .unwrap()
        .join()
        .unwrap();
    println!("\nOUTCOME {outcome}");
    std::process::exit(0);
}

/// The parent's half: run one probe in a child, killing it at [`DEADLINE`], and return how
/// long the encode took and how much Turtle it answered.
fn probe(depth: usize) -> (Duration, usize) {
    let started = Instant::now();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE, depth.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > DEADLINE {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("encoding a {ATOMS}-atom list under {depth} lists ran past {DEADLINE:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Reaped above; `wait_with_output` now only drains the pipes. (Small output: the child
    // prints one line, so the pipes cannot fill and stall it before it exits.)
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the depth-{depth} encoder probe died: {} — {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let at = stdout
        .find("\nOUTCOME ")
        .unwrap_or_else(|| panic!("the depth-{depth} encoder probe reported nothing: {stdout}"));
    let line = stdout[at + "\nOUTCOME ".len()..].lines().next().unwrap_or("");
    let mut words = line.splitn(3, ' ');
    let status = words.next().unwrap_or("");
    let micros: u64 = words.next().unwrap_or("").parse().unwrap();
    let rest = words.next().unwrap_or("");
    assert_eq!(status, "ok", "depth {depth}: {line}");
    (Duration::from_micros(micros), rest.parse().unwrap())
}

#[test]
fn the_encoder_is_linear_in_the_datum_not_in_its_nesting_depth() {
    let (shallow, shallow_bytes) = probe(1);
    let (deep, deep_bytes) = probe(DEEP);
    // The deep document is the shallow one plus 510 single-element lists, two statements each:
    // the work grows by about that, which is small beside the 4,096-atom list.
    assert!(deep_bytes > shallow_bytes);
    assert!(
        deep < CEILING,
        "a {ATOMS}-atom list under {DEEP} lists took {deep:?} to encode (under 1 list: \
         {shallow:?}); the ceiling is {CEILING:?}"
    );
}
