# Initial conformance fixtures

> These are the executable 0.10 baseline fixtures. The accepted but not yet
> implemented 0.11 ownership cases live separately in
> `tests/fixtures/owned_model_design/` and become active only through their
> ordered roadmap milestones.

Each numbered directory owns the positive run-pass fixture for the matching
top-level `docs/spec.md` section. The broader test suite supplies the associated
negative, trap, and interaction layers listed below; names are stable evidence
for the Milestone 19 ledger audit.

| Section | Positive fixture | Negative/compile-fail evidence | Trap/runtime evidence | Interaction evidence |
| --- | --- | --- | --- | --- |
| §1 Overview | `01_overview` | descriptive section; no rejected form | no trap boundary | authoritative demo |
| §2 Program layout | `02_program_layout` | lexer, parser, package-graph, and resolution suites | no trap boundary | `public_reexport_chain_preserves_the_original_identity` |
| §3 Values/references | `03_values_references` | `checks_assignment_place_mutability`, raw/reference conversion diagnostics | raw null/alignment trap tests | logical-copy and reference-promotion backend suites |
| §4 Types | `04_types` | type, construction, literal, and collection compile-fail suites | numeric/index/key trap tests | generic enum/collection backend suites |
| §5 Functions | `05_functions` | call, receiver, and function-signature compile-fail suites | null raw-function trap | methods/function-reference backend suite |
| §6 Generics/traits | `06_generics_traits` | inference, conformance, coherence, and object-safety suites | no distinct trap boundary | generic trait and dynamic-dispatch backend suites |
| §7 Expressions/control flow | `07_control_flow` | expression, pattern, exhaustiveness, iterator-obligation, and placement suites | arithmetic/index trap suites | evaluation-order, user-iterator, and match-lowering suites |
| §8 Errors/cleanup | `08_errors_cleanup` | propagation and deferred-control compile-fail suites | cleanup/trap termination suite | propagation and cleanup-order backend suites |
| §9 Garbage collection | `09_gc` | reference-formation and promotion suites | OOM collect/retry/terminate generated-C test; best-effort churn test | recursive graph and allocation-churn backend suites |
| §10 Unsafe/C ABI | `10_unsafe` | unsafe, pointer-validity, and FFI contract suites | raw-pointer and callback trap processes | `examples/c_ffi` and C harness/callback integration tests |
| §11 Conformance example | `11_example` | earlier section suites own its forms | earlier trap suites | exact normative `Counter` behavior |
| §10.4 Native concurrency | `14_concurrency` | shared-alias capture and callable-shape suites | self-join and worker-panic processes | registered callback, lifecycle, channel, mutex, and atomic tests |

`12_runtime_stress` is the M19 cross-section stress layer. It combines generic
instantiation, recursive calls, collection churn and shallow copying, managed cycles,
an escaped local reference, and nested cleanup registrations. Callback reentry
is owned by `foreign_callbacks_retain_registered_context_until_close` in
`tests/backend.rs`, while the C ABI interaction fixture remains
`examples/c_ffi`.

`13_target_width` owns the target-specific matrix output. The conformance
runner prefers `expected.<target>.stdout` (and corresponding stderr/status
files) when present, so x86 and x86-64 must demonstrate their distinct
`isize`/`usize` widths instead of merely accepting a target flag.

`14_concurrency` is the deterministic concurrency contract fixture. It covers
Elamite-thread C callback reentry, worker-held runtime values, shallow
publication aliases, repeated joins, ordinary `Result` and `defer` behavior,
channel state distinctions and closure, mutex copies, and the complete atomic
surface. `15_concurrency_stress` is its repeated four-producer/four-consumer
contention layer; consumers run until explicit closure so the test never
assumes a scheduler-specific distribution of messages. It also publishes
ordinary shared vector backing across thread creation, channel, mutex, atomic,
and join edges and checks a coordinated store-buffering litmus against the
single sequentially consistent atomic order. The fixture runs under TSan;
deliberately racy examples remain compile-only and are never conformance runs.

Run all positive fixtures with:

```sh
cargo run -- conformance tests/fixtures/conformance
```
