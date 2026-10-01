//! **Arrangements as s-expressions: lossless is a test, not a claim.**
//!
//! For each kernel K below — one covering every kind a declaration can rebuild, and one in
//! the shape of the tutorial's tic-tac-toe game — the arrangement goes around every loop:
//!
//! - `Topology → s-expression → Topology` is the identity, and the printed text is pinned;
//! - `s-expression → Turtle → s-expression` prints the same text, and the Turtle is exactly
//!   what core renders for K;
//! - the space core builds from the s-expression answers every sampled name as K does.
//!
//! Then the refusals — each naming what and where — the bounds, which are core's and are
//! held to core's own refusal on every shape, and the host's path: a lossless plan from
//! `text/x-ikigai-arrangement` to `text/turtle` that picks this crate's transreptor, over
//! `arrangement_space()` alone and (with the `full` feature) beside every other surface.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    build, select_transreptor, Alias, AliasTable, ArgRef, AsyncFnEndpoint, Capability, Confine,
    DeclarationBound, DeclarationError, Door, Endpoint, EndpointSpace, Exact, Fallback, FnEndpoint,
    Iri, Kernel, Level, Limit, MatchKind, Mount, Registry, ReprType, Representation, Request,
    RuleKind, Space, SpaceKind, Topology, TopologyRule, UriTemplate, Verb, DEFAULT_MAX_HOPS,
    MAX_DECLARATION_DEPTH, MAX_DECLARATION_NODES, MAX_DECLARATION_TEXT,
};
#[cfg(feature = "full")]
use ikigai_core::{select_transreptor_with, TransreptionPolicy};
use ikigai_sexpr::{
    arrangement_space, arrangement_to_topology, arrangement_to_turtle, topology_to_arrangement,
    turtle_to_arrangement, ArrangementError, MEDIA_ARRANGEMENT, MEDIA_TURTLE,
};
#[cfg(feature = "full")]
use ikigai_sexpr::{space, MEDIA_SEXPR};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// An endpoint that answers its own name and the bindings it was resolved with.
fn says(name: &'static str) -> Arc<dyn Endpoint> {
    Arc::new(FnEndpoint::new(name, move |inv| {
        let mut bindings: Vec<String> = inv
            .bindings
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        bindings.sort();
        Ok(Representation::new(
            ReprType::new("text/plain"),
            format!("{name} {}", bindings.join(",")).into_bytes(),
        )
        .cacheable())
    }))
}

/// An endpoint that answers by reading `target` — so where its sub-request resolves (a
/// level, a confined corridor) is part of what it answers.
fn reads(name: &'static str, target: &'static str) -> Arc<dyn Endpoint> {
    Arc::new(AsyncFnEndpoint::new(name, move |inv| {
        Box::pin(async move { inv.source(&Iri::parse(target).unwrap()).await })
    }))
}

fn registry(endpoints: &[&Arc<dyn Endpoint>]) -> Registry {
    let mut registry = Registry::new();
    for endpoint in endpoints {
        registry.register(Arc::clone(endpoint)).unwrap();
    }
    registry
}

/// What a kernel over `space` answers for `name`: the bytes, or the refusal's text.
fn answer(kernel: &Kernel, name: &str) -> String {
    let request = Request::new(Verb::Source, iri(name));
    match block_on(kernel.issue(request, &Capability::root())) {
        Ok(representation) => String::from_utf8_lossy(&representation.bytes).into_owned(),
        Err(e) => format!("refused: {e}"),
    }
}

/// Every loop, for one coded space, its registry, its pinned canonical text, and names to
/// ask.
fn round_trip(coded: Arc<dyn Space>, registry: &Registry, text: &str, names: &[&str]) {
    let tree = coded.topology();

    // Topology → s-expression is the pinned text, and back is the identity.
    assert_eq!(topology_to_arrangement(&tree).unwrap(), text);
    assert_eq!(arrangement_to_topology(text).unwrap(), tree);

    // s-expression → Turtle is exactly core's rendering, and back prints the same text.
    let turtle = arrangement_to_turtle(text).unwrap();
    assert_eq!(turtle, tree.to_turtle());
    assert_eq!(turtle_to_arrangement(&turtle).unwrap(), text);

    // The space built from the s-expression answers every name as the coded one does.
    let declared = build(&arrangement_to_topology(text).unwrap(), registry).unwrap();
    assert_eq!(declared.topology(), tree);
    let (k, k2) = (Kernel::new(coded), Kernel::new(declared));
    for name in names {
        assert_eq!(answer(&k2, name), answer(&k, name), "{name}");
    }
}

// ---- every kind ------------------------------------------------------------------------

/// Every rebuildable kind, named and anonymous: three limiters (prefix, exact, template), a
/// mount over a sealing level, a level in a namespace the host accepted, an alias with a hop
/// bound over a leaf with a shadowed door and two doors confined to ONE named corridor, and a
/// leaf whose doors need `:match` (a template with no variables, an exact name with a brace).
fn every_kind() -> (Arc<dyn Space>, Registry) {
    let public = reads("mod-public", "urn:test:internal:helper");
    let helper = says("mod-helper");
    let ns_thing = says("ns-thing");
    let thing = says("thing");
    let open = says("open-any");
    let shadowed = says("shadowed");
    let extract = reads("extract", "urn:test:ctx:doc:body");
    let leak = reads("leak", "urn:test:open:thing");
    let body = says("body");
    let private = says("private");
    let doc = says("doc");
    let plain = says("plain");
    let braced = says("a name with spaces");
    let registry = registry(&[
        &public, &helper, &ns_thing, &thing, &open, &shadowed, &extract, &leak, &body, &private,
        &doc, &plain, &braced,
    ]);

    let corridor = || -> Arc<dyn Space> {
        Arc::new(
            EndpointSpace::new()
                .bind_arc(Exact::new("urn:test:ctx:doc:body"), Arc::clone(&body))
                .named(iri("urn:test:ctx:doc")),
        )
    };
    let module = EndpointSpace::new()
        .bind_arc(Exact::new("urn:test:mod:public"), Arc::clone(&public))
        .bind_arc(Exact::new("urn:test:internal:helper"), Arc::clone(&helper));
    let table = AliasTable::new()
        .prefix("urn:test:old:", "urn:test:open:")
        .exact("urn:test:short", "urn:test:open:thing")
        .with_max_hops(3);
    let open_space = EndpointSpace::new()
        .bind_arc(Exact::new("urn:test:open:thing"), Arc::clone(&thing))
        .bind_arc(
            UriTemplate::parse("urn:test:open:{name}").unwrap(),
            Arc::clone(&open),
        )
        // Never reached: the first door wins. Order is meaning.
        .bind_arc(Exact::new("urn:test:open:thing"), Arc::clone(&shadowed))
        .bind_arc(
            Exact::new("urn:test:extract"),
            Arc::new(Confine::new(
                iri("urn:test:ctx:doc"),
                corridor(),
                Arc::clone(&extract),
            )),
        )
        .bind_arc(
            Exact::new("urn:test:leak"),
            Arc::new(Confine::new(
                iri("urn:test:ctx:doc"),
                corridor(),
                Arc::clone(&leak),
            )),
        )
        .named(iri("urn:test:space:open"));
    let root: Arc<dyn Space> = Arc::new(
        Fallback::new(vec![
            Arc::new(Limit::new("urn:test:secret:")),
            Arc::new(
                Limit::matching(Exact::new("urn:test:open:hidden"))
                    .named(iri("urn:test:space:hide")),
            ),
            Arc::new(Limit::matching(
                UriTemplate::parse("urn:test:doc:{id}:private").unwrap(),
            )),
            Arc::new(Mount::new(
                "urn:test:mod:",
                Arc::new(
                    Level::new(iri("urn:test:level:mod"), Arc::new(module))
                        .sealing(["urn:test:mod:sealed:"]),
                ),
            )),
            Arc::new(
                Level::new(
                    iri("urn:test:level:ns"),
                    Arc::new(
                        EndpointSpace::new()
                            .bind_arc(Exact::new("urn:test:ns:x:thing"), Arc::clone(&ns_thing)),
                    ),
                )
                .sealing(["urn:test:ns:x:", "urn:test:ns:y:"])
                .in_namespace("urn:test:ns:"),
            ),
            Arc::new(
                Alias::new(Arc::new(table), Arc::new(open_space))
                    .named(iri("urn:test:space:alias")),
            ),
            Arc::new(
                EndpointSpace::new()
                    .bind_arc(
                        UriTemplate::parse("urn:test:doc:{id}:private").unwrap(),
                        Arc::clone(&private),
                    )
                    .bind_arc(
                        UriTemplate::parse("urn:test:doc:{id}").unwrap(),
                        Arc::clone(&doc),
                    )
                    .bind_arc(UriTemplate::parse("urn:test:plain").unwrap(), plain)
                    .bind_arc(Exact::new("urn:other:{braced}"), braced),
            ),
        ])
        .named(iri("urn:test:space:root")),
    );
    (root, registry)
}

const EVERY_KIND: &str = r#"(fallback :id "urn:test:space:root"
  (limit "urn:test:secret:")
  (limit "urn:test:open:hidden" :id "urn:test:space:hide" :match exact)
  (limit "urn:test:doc:{id}:private")
  (mount "urn:test:mod:"
    (level "urn:test:level:mod" :seals ("urn:test:mod:sealed:")
      (endpoints
        (door "urn:test:mod:public" mod-public)
        (door "urn:test:internal:helper" mod-helper))))
  (level "urn:test:level:ns" :seals ("urn:test:ns:x:" "urn:test:ns:y:") :namespace "urn:test:ns:"
    (endpoints
      (door "urn:test:ns:x:thing" ns-thing)))
  (alias :id "urn:test:space:alias" :max-hops 3
    (exact "urn:test:short" "urn:test:open:thing")
    (prefix "urn:test:old:" "urn:test:open:")
    (endpoints :id "urn:test:space:open"
      (door "urn:test:open:thing" thing)
      (door "urn:test:open:{name}" open-any)
      (door "urn:test:open:thing" shadowed)
      (door "urn:test:extract" extract :confined
        (endpoints :id "urn:test:ctx:doc"
          (door "urn:test:ctx:doc:body" body)))
      (door "urn:test:leak" leak :confined
        (ref "urn:test:ctx:doc"))))
  (endpoints
    (door "urn:test:doc:{id}:private" private)
    (door "urn:test:doc:{id}" doc)
    (door "urn:test:plain" plain :match template)
    (door "urn:other:{braced}" "a name with spaces" :match exact)))
"#;

#[test]
fn every_kind_goes_around_every_loop() {
    let (coded, registry) = every_kind();
    round_trip(
        coded,
        &registry,
        EVERY_KIND,
        &[
            "urn:test:secret:x",
            "urn:test:open:hidden",
            "urn:test:doc:7:private",
            "urn:test:mod:public",
            "urn:test:internal:helper",
            "urn:test:mod:sealed:x",
            "urn:test:ns:x:thing",
            "urn:test:old:thing",
            "urn:test:old:other",
            "urn:test:short",
            "urn:test:open:thing",
            "urn:test:open:other",
            "urn:test:extract",
            "urn:test:leak",
            "urn:test:ctx:doc:body",
            "urn:test:doc:7",
            "urn:test:plain",
            "urn:test:nothing",
        ],
    );
}

// ---- tic-tac-toe -----------------------------------------------------------------------

const T: &str = "urn:iki:tutorial:ttt:";

/// The shape `crates/tic-tac-toe` in ikigai-tutorial composes (`space_with_store`): every
/// door bound through `UriTemplate::parse`, so the eight without a variable are templates
/// too; the eight lines of the board as exact aliases onto `cells:{list}`; over a fallback of
/// the composites and the store.
fn tic_tac_toe() -> (Arc<dyn Space>, Registry) {
    let doors: [(&str, &'static str); 17] = [
        ("cell:{x}:{y}", "ttt-cell"),
        ("cells:{list}", "ttt-cells"),
        ("board", "ttt-board"),
        ("checkset:{x}:{y}", "ttt-checkset"),
        ("winner", "ttt-winner"),
        ("turn", "ttt-turn"),
        ("move:{x}:{y}", "ttt-move"),
        ("reset", "ttt-reset"),
        ("template:{name}", "ttt-template"),
        ("view:board", "ttt-view-board"),
        ("view:square:{x}:{y}", "ttt-view-square"),
        ("view:status", "ttt-view-status"),
        ("view:reply", "ttt-view-reply"),
        ("view:game:{game}", "ttt-view-game"),
        ("view:play:{x}:{y}", "ttt-view-play"),
        ("view:reset", "ttt-view-reset"),
        ("stored:{x}:{y}", "ttt-stored"),
    ];
    let mut endpoints: Vec<Arc<dyn Endpoint>> = Vec::new();
    let mut composites = EndpointSpace::new();
    let mut store = EndpointSpace::new();
    for (tail, name) in doors {
        let endpoint = says(name);
        let template = UriTemplate::parse(format!("{T}{tail}")).unwrap();
        if name == "ttt-stored" {
            store = store.bind_arc(template, Arc::clone(&endpoint));
        } else {
            composites = composites.bind_arc(template, Arc::clone(&endpoint));
        }
        endpoints.push(endpoint);
    }
    let lines = [
        ("row:0", "0.0,1.0,2.0"),
        ("row:1", "0.1,1.1,2.1"),
        ("row:2", "0.2,1.2,2.2"),
        ("column:0", "0.0,0.1,0.2"),
        ("column:1", "1.0,1.1,1.2"),
        ("column:2", "2.0,2.1,2.2"),
        ("diagonal:0", "0.0,1.1,2.2"),
        ("diagonal:1", "2.0,1.1,0.2"),
    ];
    let mut table = AliasTable::new();
    for (line, cells) in lines {
        table = table.exact(format!("{T}{line}"), format!("{T}cells:{cells}"));
    }
    let root: Arc<dyn Space> = Arc::new(Alias::new(
        Arc::new(table),
        Arc::new(Fallback::new(vec![Arc::new(composites), Arc::new(store)])),
    ));
    let refs: Vec<&Arc<dyn Endpoint>> = endpoints.iter().collect();
    (root, registry(&refs))
}

/// The game's space as the printer writes it. The eight lines come out in the order core's
/// alias table holds them (most specific first: the longest name first), not the order the
/// tutorial writes them in — that order says nothing, since the table sorts on insert.
const TIC_TAC_TOE: &str = r#"(alias
  (exact "urn:iki:tutorial:ttt:diagonal:0" "urn:iki:tutorial:ttt:cells:0.0,1.1,2.2")
  (exact "urn:iki:tutorial:ttt:diagonal:1" "urn:iki:tutorial:ttt:cells:2.0,1.1,0.2")
  (exact "urn:iki:tutorial:ttt:column:0" "urn:iki:tutorial:ttt:cells:0.0,0.1,0.2")
  (exact "urn:iki:tutorial:ttt:column:1" "urn:iki:tutorial:ttt:cells:1.0,1.1,1.2")
  (exact "urn:iki:tutorial:ttt:column:2" "urn:iki:tutorial:ttt:cells:2.0,2.1,2.2")
  (exact "urn:iki:tutorial:ttt:row:0" "urn:iki:tutorial:ttt:cells:0.0,1.0,2.0")
  (exact "urn:iki:tutorial:ttt:row:1" "urn:iki:tutorial:ttt:cells:0.1,1.1,2.1")
  (exact "urn:iki:tutorial:ttt:row:2" "urn:iki:tutorial:ttt:cells:0.2,1.2,2.2")
  (fallback
    (endpoints
      (door "urn:iki:tutorial:ttt:cell:{x}:{y}" ttt-cell)
      (door "urn:iki:tutorial:ttt:cells:{list}" ttt-cells)
      (door "urn:iki:tutorial:ttt:board" ttt-board :match template)
      (door "urn:iki:tutorial:ttt:checkset:{x}:{y}" ttt-checkset)
      (door "urn:iki:tutorial:ttt:winner" ttt-winner :match template)
      (door "urn:iki:tutorial:ttt:turn" ttt-turn :match template)
      (door "urn:iki:tutorial:ttt:move:{x}:{y}" ttt-move)
      (door "urn:iki:tutorial:ttt:reset" ttt-reset :match template)
      (door "urn:iki:tutorial:ttt:template:{name}" ttt-template)
      (door "urn:iki:tutorial:ttt:view:board" ttt-view-board :match template)
      (door "urn:iki:tutorial:ttt:view:square:{x}:{y}" ttt-view-square)
      (door "urn:iki:tutorial:ttt:view:status" ttt-view-status :match template)
      (door "urn:iki:tutorial:ttt:view:reply" ttt-view-reply :match template)
      (door "urn:iki:tutorial:ttt:view:game:{game}" ttt-view-game)
      (door "urn:iki:tutorial:ttt:view:play:{x}:{y}" ttt-view-play)
      (door "urn:iki:tutorial:ttt:view:reset" ttt-view-reset :match template))
    (endpoints
      (door "urn:iki:tutorial:ttt:stored:{x}:{y}" ttt-stored))))
"#;

const TIC_TAC_TOE_NAMES: [&str; 17] = [
    "urn:iki:tutorial:ttt:board",
    "urn:iki:tutorial:ttt:winner",
    "urn:iki:tutorial:ttt:turn",
    "urn:iki:tutorial:ttt:reset",
    "urn:iki:tutorial:ttt:cell:1:2",
    "urn:iki:tutorial:ttt:cells:0.0,1.0",
    "urn:iki:tutorial:ttt:row:0",
    "urn:iki:tutorial:ttt:column:2",
    "urn:iki:tutorial:ttt:diagonal:1",
    "urn:iki:tutorial:ttt:stored:1:1",
    "urn:iki:tutorial:ttt:move:0:2",
    "urn:iki:tutorial:ttt:view:board",
    "urn:iki:tutorial:ttt:view:square:0:1",
    "urn:iki:tutorial:ttt:view:game:abc",
    "urn:iki:tutorial:ttt:view:play:1:1",
    "urn:iki:tutorial:ttt:template:board",
    "urn:iki:tutorial:ttt:row:3",
];

#[test]
fn tic_tac_toe_goes_around_every_loop() {
    let (coded, registry) = tic_tac_toe();
    round_trip(coded, &registry, TIC_TAC_TOE, &TIC_TAC_TOE_NAMES);
}

/// The same game as an author writes it: comments, an endpoint name in quotes, an option
/// ahead of the positional parts, an inferred `:match` written out anyway.
const TIC_TAC_TOE_AUTHORED: &str = r#"
; The game's space is a file. Only the endpoints are code.
(alias
  ; the eight lines of the board, named onto the cells they hold
  (exact "urn:iki:tutorial:ttt:row:0"      "urn:iki:tutorial:ttt:cells:0.0,1.0,2.0")
  (exact "urn:iki:tutorial:ttt:row:1"      "urn:iki:tutorial:ttt:cells:0.1,1.1,2.1")
  (exact "urn:iki:tutorial:ttt:row:2"      "urn:iki:tutorial:ttt:cells:0.2,1.2,2.2")
  (exact "urn:iki:tutorial:ttt:column:0"   "urn:iki:tutorial:ttt:cells:0.0,0.1,0.2")
  (exact "urn:iki:tutorial:ttt:column:1"   "urn:iki:tutorial:ttt:cells:1.0,1.1,1.2")
  (exact "urn:iki:tutorial:ttt:column:2"   "urn:iki:tutorial:ttt:cells:2.0,2.1,2.2")
  (exact "urn:iki:tutorial:ttt:diagonal:0" "urn:iki:tutorial:ttt:cells:0.0,1.1,2.2")
  (exact "urn:iki:tutorial:ttt:diagonal:1" "urn:iki:tutorial:ttt:cells:2.0,1.1,0.2")
  (fallback
    (endpoints                                    ; the composites
      (door "urn:iki:tutorial:ttt:cell:{x}:{y}" ttt-cell :match template)
      (door "urn:iki:tutorial:ttt:cells:{list}" "ttt-cells")
      (door :match template "urn:iki:tutorial:ttt:board" ttt-board)
      (door "urn:iki:tutorial:ttt:checkset:{x}:{y}" ttt-checkset)
      (door "urn:iki:tutorial:ttt:winner" ttt-winner :match template)
      (door "urn:iki:tutorial:ttt:turn" ttt-turn :match template)
      (door "urn:iki:tutorial:ttt:move:{x}:{y}" ttt-move)
      (door "urn:iki:tutorial:ttt:reset" ttt-reset :match template)
      (door "urn:iki:tutorial:ttt:template:{name}" ttt-template)
      (door "urn:iki:tutorial:ttt:view:board" ttt-view-board :match template)
      (door "urn:iki:tutorial:ttt:view:square:{x}:{y}" ttt-view-square)
      (door "urn:iki:tutorial:ttt:view:status" ttt-view-status :match template)
      (door "urn:iki:tutorial:ttt:view:reply" ttt-view-reply :match template)
      (door "urn:iki:tutorial:ttt:view:game:{game}" ttt-view-game)
      (door "urn:iki:tutorial:ttt:view:play:{x}:{y}" ttt-view-play)
      (door "urn:iki:tutorial:ttt:view:reset" ttt-view-reset :match template))
    (endpoints                                    ; the store
      (door "urn:iki:tutorial:ttt:stored:{x}:{y}" ttt-stored))))
"#;

#[test]
fn an_authored_file_is_the_same_arrangement_and_prints_canonically() {
    let (coded, registry) = tic_tac_toe();
    let tree = arrangement_to_topology(TIC_TAC_TOE_AUTHORED).unwrap();
    assert_eq!(tree, coded.topology());
    // Through Turtle and back, the file comes out in canonical form: everything it means,
    // none of its comments.
    let turtle = arrangement_to_turtle(TIC_TAC_TOE_AUTHORED).unwrap();
    let back = turtle_to_arrangement(&turtle).unwrap();
    assert_eq!(back, TIC_TAC_TOE);
    assert!(
        !back.contains(';'),
        "a comment is the one thing the round trip cannot keep"
    );
    // And it builds the game.
    let (k, k2) = (
        Kernel::new(coded),
        Kernel::new(build(&tree, &registry).unwrap()),
    );
    for name in TIC_TAC_TOE_NAMES {
        assert_eq!(answer(&k2, name), answer(&k, name), "{name}");
    }
}

#[test]
fn equivalent_spellings_are_one_arrangement() {
    let canonical = concat!(
        "(fallback\n",
        "  (limit \"urn:a:\" :id \"urn:x:limit\")\n",
        "  (mount \"urn:m:\"\n",
        "    (ref \"urn:x:limit\"))\n",
        "  (endpoints\n",
        "    (door \"urn:e\" \"42\")\n",
        "    (door \"urn:f\" \":colon\")\n",
        "    (door \"urn:g\" \"\")))\n",
    );
    let respelled = concat!(
        "(fallback (limit :id \"urn:x:limit\" \"urn:a:\" :match prefix)",
        " (mount \"urn:m:\" (limit \"urn:a:\" :id \"urn:x:limit\"))", // restated, identical
        " (endpoints (door \"urn:e\" \"42\" :match exact) (door \"urn:f\" \":colon\")",
        " (door \"urn:g\" \"\")))",
    );
    assert_eq!(
        arrangement_to_topology(respelled).unwrap(),
        arrangement_to_topology(canonical).unwrap()
    );
    let turtle = arrangement_to_turtle(respelled).unwrap();
    assert_eq!(turtle_to_arrangement(&turtle).unwrap(), canonical);
    assert_eq!(
        topology_to_arrangement(&arrangement_to_topology(respelled).unwrap()).unwrap(),
        canonical
    );
}

/// An alias table is consulted most specific first whatever order its rules were added in,
/// so a declaration's rules are read in the order core's `AliasTable` holds them — checked
/// here against a real table, ties included (two rules for one name: the first added wins).
#[test]
fn alias_rules_are_read_in_the_order_core_holds_them() {
    let written = [
        ("prefix", "urn:a:", "urn:z:"),
        ("exact", "urn:a:b", "urn:z:1"),
        ("prefix", "urn:a:b", "urn:z:2"),
        ("exact", "urn:a:b", "urn:z:3"), // ties with the first exact `urn:a:b`
        ("exact", "urn:c", "urn:z:4"),
        ("prefix", "urn:long:prefix:", "urn:z:5"),
        ("exact", "urn:b", "urn:z:6"),
    ];
    let text = format!(
        "(alias {} (fallback))",
        written
            .iter()
            .map(|(kind, from, to)| format!("({kind} \"{from}\" \"{to}\")"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let table = written
        .iter()
        .fold(AliasTable::new(), |table, (kind, from, to)| match *kind {
            "exact" => table.exact(*from, *to),
            _ => table.prefix(*from, *to),
        });
    let coded = Alias::new(Arc::new(table), Arc::new(Fallback::new(vec![])));
    assert_eq!(arrangement_to_topology(&text).unwrap(), coded.topology());
}

// ---- refusals --------------------------------------------------------------------------

/// Read `src`, expecting a refusal at `at` whose reason contains `because`.
fn refused(src: &str, at: &str, because: &str) {
    let error: ArrangementError = arrangement_to_topology(src)
        .map(|t| panic!("{src} was accepted as {t:?}"))
        .unwrap_err();
    assert_eq!(error.at, at, "{src}: {error}");
    assert!(
        error.reason.contains(because),
        "{src}: expected `{because}` in: {error}"
    );
    // The transreptor refuses it the same way.
    assert_eq!(arrangement_to_turtle(src).unwrap_err(), error);
}

#[test]
fn an_unknown_form_or_option_is_refused_where_it_is() {
    refused(
        "(tunnel \"urn:x:\")",
        "root (tunnel)",
        "unknown form `(tunnel …)`",
    );
    refused(
        "(fallback (limit \"urn:a:\") (tunnel))",
        "root (fallback) › layer 2 (tunnel)",
        "unknown form",
    );
    refused(
        "(fallback :colour \"red\")",
        "root (fallback)",
        "unknown option `:colour`: (fallback …) takes `:id`",
    );
    refused(
        "(endpoints (door \"urn:x\" x :via y))",
        "root (endpoints) › door 1",
        "unknown option `:via`",
    );
    refused(
        "(endpoints (limit \"urn:x:\"))",
        "root (endpoints) › door 1",
        "holds only `(door \"pattern\" endpoint)` forms",
    );
    refused(
        "(fallback (door \"urn:x\" x))",
        "root (fallback) › layer 1 (door)",
        "belongs inside `(endpoints …)`",
    );
    refused(
        "(alias (exact \"urn:a\" \"urn:b\") (fallback) (exact \"urn:c\" \"urn:d\"))",
        "root (alias)",
        "missing the space",
    );
    refused("\"urn:x\"", "root", "expected a form");
    refused("()", "root", "an empty form");
    refused("(\"endpoints\")", "root", "a form starts with its kind");
}

#[test]
fn a_kind_core_refuses_is_refused_by_name() {
    refused("(opaque)", "root (opaque)", "remote peer");
    refused(
        "(mount \"urn:x:\" (opaque))",
        "root (mount) › space (opaque)",
        "#630",
    );
    refused(
        "(rewrite (fallback))",
        "root (rewrite)",
        "is an `(alias …)`",
    );
    refused(
        "(chain (fallback))",
        "root (chain)",
        "declare the root layer",
    );
    refused(
        "(confine (fallback))",
        "root (confine)",
        "`:confined` corridor",
    );
    refused(
        "(endpoints (door \"urn:x:\" x :match prefix))",
        "root (endpoints) › door 1",
        "a door is an exact name or a template",
    );
    refused(
        "(endpoints (door \"urn:x\" x :match custom))",
        "root (endpoints) › door 1",
        "custom",
    );
    refused("(limit \"urn:x\" :match custom)", "root (limit)", "custom");
    refused(
        "(limit \"urn:x\" :match regex)",
        "root (limit)",
        "not a kind of match",
    );
}

#[test]
fn a_malformed_pattern_or_identity_is_refused() {
    refused(
        "(endpoints (door \"urn:x:{unclosed\" x))",
        "root (endpoints) › door 1",
        "is not a URI template",
    );
    refused(
        "(limit \"urn:x:{a}{\")",
        "root (limit)",
        "is not a URI template",
    );
    refused(
        "(fallback :id \"not an iri\")",
        "root (fallback)",
        "is not an IRI",
    );
    refused(
        "(fallback :id \"urn:ikigai:space:_:3\")",
        "root (fallback)",
        "would read back as anonymous",
    );
    refused(
        "(level \"urn:ikigai:space:_:1\" (fallback))",
        "root (level)",
        "would read back as anonymous",
    );
    refused(
        "(endpoints (door \"urn:x\" 42))",
        "root (endpoints) › door 1",
        "an endpoint name is a symbol, or a string",
    );
}

#[test]
fn a_part_missing_extra_or_stated_twice_is_refused() {
    refused(
        "(mount \"urn:x:\")",
        "root (mount)",
        "takes a prefix and one space; found 1 part",
    );
    refused(
        "(mount \"urn:x:\" (fallback) (fallback))",
        "root (mount)",
        "found 3 parts",
    );
    refused("(mount (fallback))", "root (mount)", "found 1 part");
    refused(
        "(mount (fallback) \"urn:x:\")",
        "root (mount)",
        "a mount's prefix is a string",
    );
    refused("(alias)", "root (alias)", "missing the space");
    refused(
        "(level (fallback))",
        "root (level)",
        "takes its name and one space",
    );
    refused("(limit)", "root (limit)", "takes one family");
    refused(
        "(endpoints (door \"urn:x\"))",
        "root (endpoints) › door 1",
        "takes a pattern and the name of the endpoint",
    );
    refused(
        "(alias (exact \"urn:a\") (fallback))",
        "root (alias) › rule 1",
        "takes the name it matches and the name it rewrites to",
    );
    refused(
        "(fallback :id \"urn:a\" :id \"urn:b\")",
        "root (fallback)",
        "option `:id` is stated twice",
    );
    refused(
        "(fallback :id)",
        "root (fallback)",
        "option `:id` has no value",
    );
    refused(
        "(alias :max-hops 0 (fallback))",
        "root (alias)",
        "a positive count",
    );
    refused(
        "(level \"urn:l\" :seals (\"urn:l:a:\" \"urn:l:a:\") (fallback))",
        "root (level <urn:l>)",
        "is sealed twice",
    );
    refused(
        "(level \"urn:l\" :id \"urn:m\" (fallback))",
        "root (level)",
        "unknown option `:id`",
    );
    refused(
        "(endpoints (door \"urn:x\" x :confined (endpoints)))",
        "root (endpoints) › door 1 › :confined",
        "give it an `:id`",
    );
}

#[test]
fn a_name_is_a_claim() {
    refused(
        "(fallback (ref \"urn:later\") (fallback :id \"urn:later\"))",
        "root (fallback) › layer 1 (ref <urn:later>)",
        "no space named <urn:later> is declared before this point",
    );
    refused(
        "(fallback (limit \"urn:a:\" :id \"urn:x\") (limit \"urn:b:\" :id \"urn:x\"))",
        "root (fallback) › layer 2 (limit <urn:x>)",
        "two different arrangements claim <urn:x>",
    );
    // A space cannot reference itself: it is not declared until it is complete.
    refused(
        "(fallback :id \"urn:self\" (ref \"urn:self\"))",
        "root (fallback <urn:self>) › layer 1 (ref <urn:self>)",
        "no space named <urn:self>",
    );
}

/// What the printer refuses: exactly what the grammar cannot say.
#[test]
fn the_printer_refuses_what_the_grammar_cannot_say() {
    let leaf = || Topology::new(SpaceKind::Fallback);
    let cases: Vec<(Topology, &str, &str)> = vec![
        (Topology::opaque(None), "root (opaque)", "remote peer"),
        (
            Topology::new(SpaceKind::Rewrite).child(leaf()),
            "root (rewrite)",
            "closure rewrite",
        ),
        (
            Topology::new(SpaceKind::Fallback)
                .child(Topology::new(SpaceKind::Confine).child(leaf())),
            "root (fallback) › layer 1 (confine)",
            "a confinement lives at a door",
        ),
        (
            Topology::new(SpaceKind::EndpointSpace {
                doors: vec![Door::new("urn:x:", MatchKind::Prefix, "x")],
            }),
            "root (endpoints) › door 1",
            "is a prefix door",
        ),
        (
            Topology::new(SpaceKind::EndpointSpace {
                doors: vec![Door::new("urn:x", MatchKind::Custom, "x")],
            }),
            "root (endpoints) › door 1",
            "custom grammar",
        ),
        (
            Topology::new(SpaceKind::EndpointSpace {
                doors: vec![Door::new("urn:x:{", MatchKind::Template, "x")],
            }),
            "root (endpoints) › door 1",
            "is not a URI template",
        ),
        (
            Topology::new(SpaceKind::Fallback)
                .child(leaf().with_id(Some(iri("urn:x"))))
                .child(
                    Topology::new(SpaceKind::Limit {
                        family: "urn:y:".into(),
                        kind: MatchKind::Prefix,
                    })
                    .with_id(Some(iri("urn:x"))),
                ),
            "root (fallback) › layer 2 (limit <urn:x>)",
            "two different arrangements claim <urn:x>",
        ),
        (
            Topology::new(SpaceKind::Mount {
                prefix: "urn:m:".into(),
            }),
            "root (mount)",
            "encloses 0 spaces, and a `mount` encloses 1",
        ),
    ];
    for (tree, at, because) in cases {
        let error = topology_to_arrangement(&tree).unwrap_err();
        assert_eq!(error.at, at, "{error}");
        assert!(
            error.reason.contains(because),
            "expected `{because}` in: {error}"
        );
    }
}

#[test]
fn turtle_that_is_not_an_arrangement_is_refused() {
    // Core reads the document, so the refusal is core's, kept typed.
    let error = turtle_to_arrangement("this is not turtle").unwrap_err();
    assert!(error.reason.contains("not Turtle"), "{error}");
    assert!(
        matches!(
            error.declaration.as_deref(),
            Some(DeclarationError::Malformed { .. })
        ),
        "{error}"
    );

    // A chain is what `urn:kernel:topology` answers, and not a space: core reads it, the
    // grammar cannot say it.
    let chain = Topology::new(SpaceKind::Chain { severed: false })
        .with_id(Some(iri("urn:test:chain")))
        .child(Topology::new(SpaceKind::Fallback));
    let error = turtle_to_arrangement(&chain.to_turtle()).unwrap_err();
    assert_eq!(error.at, "root (chain <urn:test:chain>)", "{error}");

    // An opaque peer under a mount, named where core would name it.
    let remote = Topology::new(SpaceKind::Mount {
        prefix: "urn:peer:".into(),
    })
    .child(Topology::opaque(None));
    let error = turtle_to_arrangement(&remote.to_turtle()).unwrap_err();
    assert_eq!(error.at, "root (mount) › space (opaque)", "{error}");

    // Core's own refusal, naming the node as the Turtle does.
    let odd = Topology::new(SpaceKind::Fallback)
        .to_turtle()
        .replace("ik:Fallback", "ik:Tunnel");
    let error = turtle_to_arrangement(&odd).unwrap_err();
    assert_eq!(error.at, "<urn:ikigai:space:_:1>", "{error}");
    assert!(error.reason.contains("ik:Tunnel"), "{error}");
}

// ---- bounds ----------------------------------------------------------------------------
//
// An arrangement is held to core's declaration bounds, in core's units, on both paths: a
// Turtle document is core's own to refuse, and the s-expression reader counts as core counts
// and refuses with core's error. Every refusal below is checked against what core itself says
// about the same tree, through `build` (which runs core's own measure).

/// The typed refusal an error carries: core's.
fn core_refusal(error: &ArrangementError) -> &DeclarationError {
    error
        .declaration
        .as_deref()
        .unwrap_or_else(|| panic!("not core's refusal: {error}"))
}

fn too_large(bound: DeclarationBound, node: &str) -> DeclarationError {
    DeclarationError::TooLarge {
        bound,
        limit: bound.limit(),
        node: node.into(),
    }
}

/// What core's `build` refuses `tree` for, before it looks for a single endpoint.
fn core_refuses(tree: &Topology) -> DeclarationError {
    match build(tree, &Registry::new()) {
        Err(e @ DeclarationError::TooLarge { .. }) => e,
        Err(other) => panic!("core passed the bounds and refused for another reason: {other}"),
        Ok(_) => panic!("core built it"),
    }
}

/// A chain `depth` spaces deep — core's units: a door is not a level, its confined corridor
/// is — that cycles through every kind that encloses: fallback, mount, alias, level, and a
/// door confined to a corridor. It ends in a limiter at exactly `depth`.
fn every_kind_nested(depth: usize) -> String {
    let (mut open, mut close) = (String::new(), String::new());
    let (mut d, mut i) = (1, 0);
    while d < depth {
        i += 1;
        let (form, closing, deeper) = match i % 5 {
            0 => ("(fallback (limit \"urn:q:\") ".to_string(), ")", 1),
            1 => (format!("(mount \"urn:m{i}:\" "), ")", 1),
            2 => ("(alias (exact \"urn:a\" \"urn:b\") ".to_string(), ")", 1),
            3 => (format!("(level \"urn:l:{i}\" "), ")", 1),
            _ if d + 2 <= depth => (
                format!("(endpoints (door \"urn:d{i}\" x :confined (fallback :id \"urn:c:{i}\" "),
                ")))",
                2,
            ),
            _ => ("(fallback ".to_string(), ")", 1),
        };
        open.push_str(&form);
        close.push_str(closing);
        d += deeper;
    }
    format!("{open}(limit \"urn:x:\"){close}")
}

fn nested_fallbacks(depth: usize) -> String {
    format!(
        "{}(limit \"urn:x:\"){}",
        "(fallback ".repeat(depth - 1),
        ")".repeat(depth - 1)
    )
}

#[test]
fn nesting_is_bounded_as_core_bounds_it_and_the_bound_itself_round_trips() {
    // Past the bound: core's typed refusal, naming the node core numbers 49.
    let error = arrangement_to_topology(&nested_fallbacks(MAX_DECLARATION_DEPTH + 1)).unwrap_err();
    assert_eq!(
        core_refusal(&error),
        &too_large(DeclarationBound::Depth, "urn:ikigai:space:_:49"),
        "{error}"
    );
    // Far past it, the crate's s-expression reader stops first.
    let error = arrangement_to_topology(&nested_fallbacks(100_000)).unwrap_err();
    assert!(error.reason.contains("reader limit"), "{error}");
    assert_eq!(error.declaration, None);

    // AT the bound, every walk — this crate's and core's to_turtle, from_turtle and build —
    // fits a 2 MiB worker stack, in the debug build CI tests.
    let worker = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let x = says("x");
            let registry = registry(&[&x]);
            for text in [
                nested_fallbacks(MAX_DECLARATION_DEPTH),
                every_kind_nested(MAX_DECLARATION_DEPTH),
            ] {
                let tree = arrangement_to_topology(&text).unwrap();
                let turtle = arrangement_to_turtle(&text).unwrap();
                let back = turtle_to_arrangement(&turtle).unwrap();
                assert_eq!(arrangement_to_topology(&back).unwrap(), tree);
                build(&tree, &registry).unwrap();
            }
        })
        .unwrap();
    worker.join().expect("the bound fits a 2 MiB stack");

    // One level past it, through every kind: the reader refuses exactly as core does — the
    // same bound, the same node — whether the tree arrives as s-expression or as a value.
    for at_bound in [
        nested_fallbacks(MAX_DECLARATION_DEPTH),
        every_kind_nested(MAX_DECLARATION_DEPTH),
    ] {
        let tree = arrangement_to_topology(&at_bound).unwrap();
        let deeper = Topology::new(SpaceKind::Fallback).child(tree);
        let expected = core_refuses(&deeper);
        let error = arrangement_to_topology(&format!("(fallback {at_bound})")).unwrap_err();
        assert_eq!(core_refusal(&error), &expected, "{error}");
        let error = topology_to_arrangement(&deeper).unwrap_err();
        assert_eq!(core_refusal(&error), &expected, "{error}");
        assert!(matches!(
            expected,
            DeclarationError::TooLarge {
                bound: DeclarationBound::Depth,
                ..
            }
        ));
    }

    // A Turtle document nested far past the bound is core's to refuse, before its recursive
    // reader descends past the bound — and core's typed refusal reaches the caller.
    let mut turtle = String::from("@prefix ik: <https://ikigai-rs.dev/ns#> .\n");
    let n = 200_000;
    for i in 1..n {
        turtle.push_str(&format!(
            "<urn:m:{i}> a ik:Mount ; ik:prefix \"urn:x:\" ; ik:space <urn:m:{}> .\n",
            i + 1
        ));
    }
    turtle.push_str(&format!(
        "<urn:m:{n}> a ik:Limit ; ik:family \"urn:x:\" ; ik:matchKind \"prefix\" .\n"
    ));
    let error = turtle_to_arrangement(&turtle).unwrap_err();
    assert_eq!(
        core_refusal(&error),
        &too_large(DeclarationBound::Depth, "urn:m:49"),
        "{error}"
    );
    assert_eq!(error.at, "<urn:m:49>");
}

/// The billion-laughs shape: each named space places the previous one twice, so `k` lines
/// stand for 2^k spaces.
fn doubling(k: usize, family: &str) -> String {
    let mut text = format!("(fallback\n  (limit \"{family}\" :id \"urn:f:0\")\n");
    for j in 1..=k {
        let p = j - 1;
        text.push_str(&format!(
            "  (fallback :id \"urn:f:{j}\" (ref \"urn:f:{p}\") (ref \"urn:f:{p}\"))\n"
        ));
    }
    text.push(')');
    text
}

/// The same tree as [`doubling`], built as a value — 2^k spaces, so only for a small `k`.
fn doubled(k: usize, family: &str) -> Topology {
    let mut named = Topology::new(SpaceKind::Limit {
        family: family.into(),
        kind: MatchKind::Prefix,
    })
    .with_id(Some(iri("urn:f:0")));
    let mut tree = Topology::new(SpaceKind::Fallback).child(named.clone());
    for j in 1..=k {
        named = Topology::new(SpaceKind::Fallback)
            .with_id(Some(iri(&format!("urn:f:{j}"))))
            .child(named.clone())
            .child(named);
        tree = tree.child(named.clone());
    }
    tree
}

#[test]
fn a_ref_cannot_multiply_the_arrangement_past_the_bound() {
    // 2^40 spaces from forty lines: refused at the first `ref` that passes 65,536 nodes,
    // named as the space it places again — exactly as core refuses the same tree (built
    // here as a value only as far as it needs to be to pass the bound).
    let error = arrangement_to_topology(&doubling(40, "urn:x:")).unwrap_err();
    assert_eq!(
        core_refusal(&error),
        &too_large(DeclarationBound::Nodes, "urn:f:14"),
        "{error}"
    );
    assert_eq!(core_refusal(&error), &core_refuses(&doubled(16, "urn:x:")));

    // The same shape as Turtle is small — core renders a named node once — and core's reader
    // would expand it at every reference. Core counts as it reads, and refuses: twenty
    // doublings, so the chain stays inside the depth bound and the node bound is the one met.
    let mut turtle = String::from(
        "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         <urn:f:0> a ik:Limit ; ik:family \"urn:x:\" ; ik:matchKind \"prefix\" .\n",
    );
    for k in 1..=20 {
        let p = k - 1;
        turtle.push_str(&format!(
            "<urn:f:{k}> a ik:Fallback ; ik:layers <urn:f:{k}:layer:1> .\n\
             <urn:f:{k}:layer:1> rdf:first <urn:f:{p}> ; rdf:rest <urn:f:{k}:layer:2> .\n\
             <urn:f:{k}:layer:2> rdf:first <urn:f:{p}> ; rdf:rest rdf:nil .\n"
        ));
    }
    let error = turtle_to_arrangement(&turtle).unwrap_err();
    assert!(
        matches!(
            core_refusal(&error),
            DeclarationError::TooLarge {
                bound: DeclarationBound::Nodes,
                limit: MAX_DECLARATION_NODES,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn text_is_bounded_too_counted_at_every_place_it_is_used() {
    // One 1 MiB family, placed 2^4 times: past MAX_DECLARATION_TEXT with a few dozen nodes.
    let family = format!("urn:x:{}", "y".repeat(1024 * 1024));
    let error = arrangement_to_topology(&doubling(4, &family)).unwrap_err();
    assert_eq!(
        core_refusal(&error),
        &core_refuses(&doubled(4, &family)),
        "{error}"
    );
    assert!(matches!(
        core_refusal(&error),
        DeclarationError::TooLarge {
            bound: DeclarationBound::Text,
            ..
        }
    ));
}

/// Shapes that grow by one unit of a bound at a time, each with the largest size core still
/// accepts: the reader accepts that one and refuses the next exactly as core refuses it. This
/// is what holds the reader's count — doors and alias rules as nodes, a mount's prefix and a
/// limiter's family as text — to core's.
#[test]
fn the_reader_counts_exactly_as_core_counts() {
    let x = says("x");
    let registry = registry(&[&x]);

    // Doors: an endpoint space is one node and each door another.
    let doors = |n: usize| {
        let doors: Vec<Door> = (0..n)
            .map(|i| Door::new(format!("urn:d:{i}"), MatchKind::Exact, "x"))
            .collect();
        let text = doors
            .iter()
            .map(|d| format!("(door \"{}\" x)", d.pattern))
            .collect::<Vec<_>>()
            .join(" ");
        (
            format!("(endpoints {text})"),
            Topology::new(SpaceKind::EndpointSpace { doors }),
        )
    };
    // Rules: an alias is one node, each rule another, and the space it encloses one more.
    let rules = |n: usize| {
        let rules: Vec<TopologyRule> = (0..n)
            .map(|i| TopologyRule::new(RuleKind::Exact, format!("urn:a:{i:06}"), "urn:b"))
            .collect();
        let text = rules
            .iter()
            .map(|r| format!("(exact \"{}\" \"urn:b\")", r.from))
            .collect::<Vec<_>>()
            .join(" ");
        let limit = Topology::new(SpaceKind::Limit {
            family: "urn:x:".into(),
            kind: MatchKind::Prefix,
        });
        (
            format!("(alias {text} (limit \"urn:x:\"))"),
            Topology::new(SpaceKind::Alias {
                rules,
                max_hops: DEFAULT_MAX_HOPS,
            })
            .child(limit),
        )
    };
    // Text: a mount's prefix and the limiter it encloses, `n` bytes between them.
    let text = |n: usize| {
        let prefix = format!("urn:{}", "p".repeat(n / 2 - 4));
        let family = format!("urn:{}", "f".repeat(n - prefix.len() - 4));
        (
            format!("(mount \"{prefix}\" (limit \"{family}\"))"),
            Topology::new(SpaceKind::Mount { prefix }).child(Topology::new(SpaceKind::Limit {
                family,
                kind: MatchKind::Prefix,
            })),
        )
    };

    type Shape<'a> = &'a dyn Fn(usize) -> (String, Topology);
    let shapes: [(&str, Shape, usize); 3] = [
        ("doors", &doors, MAX_DECLARATION_NODES - 1),
        ("rules", &rules, MAX_DECLARATION_NODES - 2),
        ("text", &text, MAX_DECLARATION_TEXT),
    ];
    for (what, shape, largest) in shapes {
        let (text, tree) = shape(largest);
        assert_eq!(arrangement_to_topology(&text).unwrap(), tree, "{what}");
        if what == "doors" {
            build(&tree, &registry).unwrap();
        }
        let (text, tree) = shape(largest + 1);
        let error = arrangement_to_topology(&text).unwrap_err();
        assert_eq!(
            core_refusal(&error),
            &core_refuses(&tree),
            "{what}: {error}"
        );
        let error = topology_to_arrangement(&tree).unwrap_err();
        assert_eq!(
            core_refusal(&error),
            &core_refuses(&tree),
            "{what}: {error}"
        );
    }
}

// ---- the host's path -------------------------------------------------------------------

/// What the ikigai host does with `--arrangement` (ikigai-cli PR #371): read the file
/// through a kernel, plan a LOSSLESS route to `text/turtle`, issue each step with the value
/// as `content` and the target as `as`, then parse and build. Run over `surface` mounted
/// beside the file.
fn host_path(surface: Arc<dyn Space>) -> Arc<dyn Space> {
    let file: Arc<dyn Endpoint> = Arc::new(FnEndpoint::new("the-file", |_| {
        Ok(Representation::new(
            ReprType::new(MEDIA_ARRANGEMENT),
            TIC_TAC_TOE_AUTHORED.as_bytes().to_vec(),
        ))
    }));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:file:game.arrangement"), file)),
        surface,
    ]));
    let kernel = Kernel::new(Arc::clone(&root));
    let issue = |request: Request| block_on(kernel.issue(request, &Capability::root())).unwrap();

    let mut current = issue(Request::new(Verb::Source, iri("urn:file:game.arrangement")));
    let plan = select_transreptor(root.as_ref(), MEDIA_ARRANGEMENT, MEDIA_TURTLE).unwrap();
    let hops: Vec<(&str, &str, bool)> = plan
        .iter()
        .map(|s| (s.endpoint.as_str(), s.to.as_str(), s.lossless))
        .collect();
    assert_eq!(hops, [("urn:sexpr:arrangement-to-rdf", MEDIA_TURTLE, true)]);
    for step in plan {
        current = issue(
            Request::new(Verb::Source, iri(&step.endpoint))
                .with_arg("content", ArgRef::Inline(current.bytes))
                .with_arg("as", ArgRef::Inline(step.to.into_bytes())),
        );
    }
    assert_eq!(current.repr_type.media_type, MEDIA_TURTLE);
    let turtle = String::from_utf8(current.bytes).unwrap();
    let (coded, registry) = tic_tac_toe();
    let tree = Topology::from_turtle(&turtle).unwrap();
    assert_eq!(tree, coded.topology());
    build(&tree, &registry).unwrap();

    // The reverse edge exists, and is lossless too.
    let back = select_transreptor(root.as_ref(), MEDIA_TURTLE, MEDIA_ARRANGEMENT).unwrap();
    assert_eq!(back[0].endpoint, "urn:sexpr:arrangement-from-rdf");
    assert!(back[0].lossless);
    root
}

/// The arrangement surface alone — what a page mounts, built without the `full` feature — is
/// everything the host's path needs, and binds nothing else.
#[test]
fn the_arrangement_space_alone_takes_an_arrangement_file_to_a_built_space() {
    let surface = arrangement_space();
    let bound: Vec<String> = match surface.topology().kind {
        SpaceKind::EndpointSpace { doors } => doors.into_iter().map(|d| d.pattern).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        bound,
        [
            "urn:sexpr:arrangement-to-rdf",
            "urn:sexpr:arrangement-from-rdf"
        ]
    );
    host_path(Arc::new(surface));
}

/// Beside every other surface, the plan still picks this crate's transreptor, and an
/// s-expression that is NOT an arrangement never reaches this crate's reader: `text/x-sexpr`
/// has no lossless route to plain Turtle at all (`urn:rdf:from-sexpr` interprets a
/// `(graph …)`, so it is declared lossy, ledger #645), and with the caller's consent it plans
/// to that one, reported lossy.
#[cfg(feature = "full")]
#[test]
fn a_lossless_plan_takes_an_arrangement_file_to_a_built_space() {
    let root = host_path(Arc::new(space()));
    assert_eq!(
        select_transreptor(root.as_ref(), MEDIA_SEXPR, MEDIA_TURTLE),
        None
    );
    let other = select_transreptor_with(
        root.as_ref(),
        MEDIA_SEXPR,
        MEDIA_TURTLE,
        &TransreptionPolicy::allow_lossy(),
    )
    .unwrap();
    let hops: Vec<(&str, bool)> = other
        .iter()
        .map(|s| (s.endpoint.as_str(), s.lossless))
        .collect();
    assert_eq!(hops, [("urn:rdf:from-sexpr", false)]);
}
