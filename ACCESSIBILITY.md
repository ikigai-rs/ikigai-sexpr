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
| Last reviewed | 2026-11-12 |

## What it emits that people read

- **Documentation**: Markdown in the README, this file and `AGENTS.md`, and Rust API
  documentation rendered by a host such as GitHub or rustdoc.
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
- Quoted input rendered by `show` in an arrangement error is shortened when it exceeds
  48 characters: the first 47 characters plus an ellipsis, enclosed in backticks.
- Nothing is written in color; every answer is plain text.

## How it is tested

Every named check has a result: Pass, Fail or Blocked. A Pass covers only the stated
check, not overall accessibility. The tests below were run on 2026-11-12 against main commit
`9f4d8fd3b822aeba0d0333851829b0016169eaed`, with no source changes, with
Cargo 1.99.1 and rustc 1.99.1 (58cf8f9c3), using `cargo test` (default features, including
doctests). CI repeats these tests through `.github/workflows/ci.yml`; they check text and
behavior, not how a host presents it. No browser or assistive technology evaluation was
performed for this review.

| Check | Result | Evidence |
|---|---|---|
| Arrangement refusal locations and reasons | Pass | `cargo test`, `tests/arrangement.rs`: `refused` asserts the location and reason for `an_unknown_form_or_option_is_refused_where_it_is`, `a_kind_core_refuses_is_refused_by_name`, `a_malformed_pattern_or_identity_is_refused`, `a_part_missing_extra_or_stated_twice_is_refused`, `the_printer_refuses_what_the_grammar_cannot_say` and `turtle_that_is_not_an_arrangement_is_refused`. Does not evaluate whether people understand the wording. |
| Depth and node bounds name the refused bound | Pass | `cargo test`, `tests/depth.rs`, `tests/dag.rs` (including `the_node_bound_is_exact_and_names_itself`) and the `parse` doctest in `src/lib.rs`. Checks refusal text and reader/decoder boundaries, not usability of large documents. |
| Malformed input returns an error | Pass | `cargo test`, `src/tests.rs`: `unbalanced_parens_are_errors` and `a_malformed_sexpr_is_a_clean_endpoint_error`. These do not assert error wording. |
| Reader text preservation, validation and resource bounds | Pass | `cargo test`, `tests/reader_audit.rs`: checks unterminated-string text bounds, quoted symbols and strings, local names and language tags, quote/comment pre-scan parity and deep reader probes in a child process on a 2 MiB thread. The pre-scan comparison models ikigai-ui; it does not run its interface. |
| Error examples, endpoint context, excerpt clipping and plain text | Blocked | No dedicated automated check of all these rules. Source review on 2026-11-12 at the main commit above: `SexprError::fmt` and `sexpr_to_turtle` in `src/lib.rs`, `show` and `ArrangementError` in `src/arrangement.rs`, and `invalid` in `src/endpoints.rs`. The source supports the rules above; this dated manual review is not repeated by CI and does not establish that every error is helpful. |
| Automated accessibility checker | Blocked | No axe or other accessibility checker was run; no rendered host page or browser checker was configured for this text library review. Rust tests are not accessibility checkers. |
| Browser presentation of documentation and emitted text | Blocked | No browser evaluation was performed. Contrast, reflow, zoom, headings and focus depend on the host and remain unverified. |
| Manual keyboard | Blocked | No person performed a keyboard pass in a host editor, terminal or documentation page during this review. |
| Screen reader | Blocked | No screen reader session was available; this output and documentation were not evaluated with a screen reader. |
| Screen magnifier | Blocked | No magnifier session was available; this output and documentation were not evaluated with a screen magnifier. |
| Speech input | Blocked | No speech-input session was available; this output and documentation were not evaluated with speech input. |
| Switch access | Blocked | No switch-access session was available; this output and documentation were not evaluated with switch access. |
| Braille display | Blocked | No braille display was available; this output and documentation were not evaluated with a braille display. |

## Known limitations

Rechecked by reading source on 2026-11-12 at the main commit above. Fail records a
known barrier in the stated check, not a WCAG conformance determination. These source
findings are not assistive technology test results.

| Check | Result | Evidence |
|---|---|---|
| Reader error location | Fail | `src/lib.rs`, reader error construction: unexpected end of input (missing a closing parenthesis), an unexpected closing parenthesis and an unterminated string have no line, column or offset. A person must search a long form by hand. |
| Library compile error surface label | Fail | `src/lib.rs`, `SexprError::fmt`: every compile error is labeled `sparql-from-sexpr:`, including errors from `sexpr_to_turtle`. The endpoints use the unlabeled detail with their own IRI and are not affected. |
| Code-graph error recovery guidance | Fail | `src/codegraph.rs`, `decode_list`: errors such as `cons node <…> has no rdf:first` identify an IRI without explaining how to repair the input. |

## Contributing

Read this file before changing any output listed above: an answer's text form or an error message.
Keep an error a sentence that says what is wrong, where, and what to do. Say in the pull request
what was tested, with what, and what was not. Give every check a row in a
`Check | Result | Evidence` table with Pass, Fail or Blocked, a test or command and tool
version, or why it could not run. Keep unrun checks as Blocked, including each assistive
technology nobody used. A historical run that CI does not repeat is Blocked with its date;
do not carry it forward as a current Pass.

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
| 2026-11-12 | Record Pass, Fail and Blocked evidence, explicit unrun checks and current main review scope. |
