# Owned-model design corpus

These fixtures are the split normative corpus for the accepted 0.11 ownership
model. They are intentionally not discovered by the current 0.10 conformance
runner. Each ordered migration milestone moves its fixtures into an active test
layer only after the required syntax and semantic pass exist; accepting the
specification must not make the compiler pretend to implement it.

| Layer | Current fixtures | Becomes active in |
| --- | --- | --- |
| Compile-pass | `pass/ownership.elx` | move, borrow, closure, and collection milestones |
| Compile-fail | `fail/use_after_move.elx`, `fail/borrow_conflict.elx` | move and provenance milestones |
| Run-pass | `run/owned_values.elx` | destruction and owned collection milestones |
| Trap | `trap/stale_handle.elx` | explicit graph ownership milestone |
| C harness | `ffi/callback.elx`, `ffi/callback_harness.c` | owned-model C interoperability |
| Target width | `target_width/handles.elx` | graph ownership plus final x86/x86-64 conformance |

The authoritative integrated target remains
`owned_spec_demo.elx`. Focused fixtures, rather than comments in that
large file, own diagnostics and runtime boundary evidence.
