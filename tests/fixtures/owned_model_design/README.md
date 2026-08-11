# Owned-model design corpus

These fixtures are the split normative corpus for the accepted 0.11 ownership
model. They are not discovered by the 0.10 conformance runner. Each ordered
migration milestone activates its applicable evidence in focused 0.11 tests
only after the required syntax and semantic pass exist; accepting the
specification must not make the compiler pretend to implement later layers.

| Layer | Current fixtures | Becomes active in |
| --- | --- | --- |
| Compile-pass | `pass/ownership.elx` | Active for move checking; borrow, closure, and collection behavior advances in their milestones |
| Compile-fail | `fail/use_after_move.elx`, `fail/borrow_conflict.elx` | Use-after-move is active; borrow conflict advances with provenance |
| Run-pass | `run/owned_values.elx` | destruction and owned collection milestones |
| Trap | `trap/stale_handle.elx` | explicit graph ownership milestone |
| C harness | `ffi/callback.elx`, `ffi/callback_harness.c` | owned-model C interoperability |
| Target width | `target_width/handles.elx` | graph ownership plus final x86/x86-64 conformance |

The authoritative integrated target remains
`owned_spec_demo.elx`. Focused fixtures, rather than comments in that
large file, own diagnostics and runtime boundary evidence.
