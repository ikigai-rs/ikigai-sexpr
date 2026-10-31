# Accessibility

What `ikigai-sexpr` emits that people read, the rules that output follows, how it is tested, and
how to report a barrier. This crate writes text: s-expressions, SPARQL, Turtle, and the text of its
errors. It writes no HTML, SVG or terminal output of its own. It is not a conformance claim.
Adapted from the [ACCESSIBILITY.md template](https://github.com/mgifford/ACCESSIBILITY.md) (MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text only |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-sexpr/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-sexpr/security/advisories/new) |
| Last reviewed | 2026-10-31 |

## What it emits that people read

- **Text answers** of the six surfaces (`src/endpoints.rs`): SPARQL from `urn:sparql:from-sexpr`,
  Turtle from `urn:rdf:from-sexpr`, `urn:sexpr:to-rdf` and `urn:sexpr:arrangement-to-rdf`, and
  s-expression text from `urn:sexpr:from-rdf` and `urn:sexpr:arrangement-from-rdf` (the printer
  in `src/lib.rs` and `src/arrangement.rs`). People write and read the s-expression and
  arrangement forms by hand.
- **Error messages**: the reader's and compilers' `SexprError` (`sexpr parse error: …`,
  `sparql-from-sexpr: …`, in `src/lib.rs` and `src/codegraph.rs`), the arrangement reader's
  `ArrangementError` (`at <where>: <reason>`, `src/arrangement.rs`), and the endpoints'
  `InvalidArgument`, which names the argument and prefixes the endpoint's IRI
  (`src/endpoints.rs`). A host shows these to whoever made the request.

Not this repository's: the page, editor or terminal a host shows any of this in.

## The rules that output follows

Followed by reading the source:

- An arrangement error says where it is, as a path through the document (`at root (endpoints) ›
  door 2: …`), and why; many also say what is allowed instead (`a space is one of endpoints,
  fallback, mount, alias, limit, level, ref`, `write (ref "…") to place the same space again`).
- A compile error often carries an example of the right form (`a graph must be a list, e.g.
  (graph (ex:s ex:p ex:o))`).
- A bound refusal names the bound and its number (nesting 512, 1,048,576 nodes, 32 MiB of text)
  and says it is the same for the reader and the code-graph decoder.
- An endpoint refusal is an `InvalidArgument` naming the argument, prefixed by the endpoint's IRI,
  so a pipeline's error says which stage refused it.
- Quoted input in an arrangement error is clipped to 47 characters, so the message stays
  readable.
- Nothing is written in color; every answer is plain text.

## How it is tested

- `tests/arrangement.rs`: `refused` checks the location (`at`) and the reason of each refusal, in
  `an_unknown_form_or_option_is_refused_where_it_is`, `a_kind_core_refuses_is_refused_by_name`,
  `a_malformed_pattern_or_identity_is_refused`, `a_part_missing_extra_or_stated_twice_is_refused`,
  `the_printer_refuses_what_the_grammar_cannot_say` and `turtle_that_is_not_an_arrangement_is_refused`.
- `tests/depth.rs` and `tests/dag.rs` (`the_node_bound_is_exact_and_names_itself`) check that a
  bound refusal names the bound; the doctest on `parse` checks the node-bound wording.
- `src/tests.rs` checks that malformed input is an error and not a panic
  (`unbalanced_parens_are_errors`, `a_malformed_sexpr_is_a_clean_endpoint_error`); it does not
  check the wording.
- **No automated accessibility checker, browser or assistive technology has been used on this
  output.**

## Known limitations

Found by reading the source on 2026-10-31, not by testing:

- The s-expression reader's errors say what is wrong but not where: ``unexpected end of input:
  missing `)` ``, ``unexpected `)` — unbalanced parens`` and `unterminated string literal` carry
  no line, column or offset, so a person must search a long form by hand.
- `SexprError`'s `Display` labels every compile error `sparql-from-sexpr:`, including errors from
  the graph compiler (`sexpr_to_turtle`), so a library caller that prints the error sees the wrong
  surface named. The endpoints use the unlabeled detail with their own IRI and are not affected.
- Several code-graph decoding errors name only an IRI (`cons node <…> has no rdf:first`) and say
  nothing about what to do.

## Contributing

Read this file before changing any output listed above: an answer's text form or an error message.
Keep an error a sentence that says what is wrong, where, and what to do. Say in the pull request
what was tested, with what, and what was not.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-sexpr/issues/new?labels=accessibility) with
the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a maintainer
adds it): what you were doing, what happened, and your terminal, editor or browser and any
assistive technology, if you are comfortable sharing them. Anything security-sensitive goes
through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-sexpr/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |
