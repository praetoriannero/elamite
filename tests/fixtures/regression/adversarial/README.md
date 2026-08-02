# Adversarial regression package

A single executable package that pushes the implemented parts of Elamite to
their specified boundaries, plus a directory of minimal reproductions for the
divergences the original audit found between `docs/SPEC.md` and the compiler.

Every divergence this package found has since been resolved, so nothing here
fails. It is kept as a behavioral lock, not a bug list.

Two halves:

- **`src/`** — a package that builds, runs, and passes its own tests. Every
  observation it prints is a rule from `docs/SPEC.md`. If a change to the compiler
  alters any line of its output, a specified behavior moved.
- **`regressions/`** — the original standalone reproductions. Resolved cases
  remain as regression fixtures; intentional rejections and known
  implementation limitations document their settled contract.

## How it runs

`cargo test` drives this package through [`tests/regression.rs`](../../../regression.rs),
which pins the full output against `expected.stdout` in both debug and
release. Nothing here needs to be run by hand, but it can be:

```sh
cargo run -- run tests/fixtures/regression/adversarial            # 411 observations
cargo run -- test tests/fixtures/regression/adversarial           # 26 trap and assertion tests
cargo run -- run tests/fixtures/regression/adversarial --release
cargo run -- run tests/fixtures/regression/adversarial --target=x86
```

One line of output is target-dependent by design: `usize bit width` reports 64
on x86-64 and 32 on x86 (`docs/SPEC.md` 4.1), so the expectation is stored twice, as
`expected.stdout` and `expected.x86.stdout`, differing on exactly that line.
Map and set iteration order is unspecified, so nothing observes it — only
order-independent aggregates.

Reproduce a single case with:

```sh
cargo run -- build tests/fixtures/regression/adversarial/regressions/variant_literal_arm.elx
```

## What `src/` covers

| Module | `docs/SPEC.md` | Boundary pushed |
| --- | --- | --- |
| `layout.elx`, `layout/` | 2.3 | inline vs file-backed modules, a directory namespace with no owning file, `super` paths, `use self.…`, re-export aliasing, circular imports within one package, package-private access across modules |
| `copies.elx` | 3.1 | recursive copy independence through five nesting levels, explicit aliases surviving a copy of their container, snapshot ordering, argument/return copies, `&var` stored in a `let` aggregate |
| `references.elx` | 3.2 | replacement of a container observed through an interior reference, mutation visible in the container, escaping locals and interior escapes, referenced composite literals, unrestricted aliasing, references in fields and enum payloads, storage-identity equality |
| `pointers.elx` | 3.3 | every safe conversion and the `*var`→`*` downgrade, `unsafe:` reads and writes, pointee-type casts, `*Self`/`*var Self` receivers, pointers in aggregates and their `null` defaults, expression-local validity below an unreachable guard |
| `numerics.elx` | 4.1 | every signed and unsigned boundary, the `checked_`/`wrapping_`/`saturating_` families at overflow, signed-minimum negation and division, shift width limits, `try_from`/`wrapping_from`/`saturating_from`, truncation toward zero, IEEE NaN and infinity, target pointer width |
| `aggregates.elx` | 4.1–4.4 | tuple shape and arity, nested destructuring, positional selectors as places, struct literal forms, recursive `Default`, all three enum variant shapes, transparent aliases, a recursive generic tree |
| `collections.elx` | 4.1, 7.1 | insertion through `len` inclusive, removal below `len`, displaced-value reporting, duplicate literal keys and elements, struct and tuple `StableHash` keys, nested collection independence, iteration as a snapshot of its source |
| `functions.elx` | 5 | explicit returns on every path, variadic tails, function-reference identity and storage, unbound method selection, `&unsafe fn`, raw function pointers, receiver adaptation |
| `closures.elx` | 5.1 | all five capture forms in one closure, capture evaluation order, snapshot vs alias semantics under closure copies, return inference, `Callable` bounds and `&Callable` erasure, escaping environments, closure-local `defer` |
| `generics.elx` | 6 | inference from arguments and from expected types, explicit argument lists, distinct instantiations, generic implementations, finite mutual recursion across instantiations, instantiated generic function references |
| `traits.elx` | 6 | inherent over trait precedence, `Type.Trait.method` qualification, default methods and overrides in the vtable, contextual and explicit trait-object conversion, heterogeneous objects, mutable trait objects, non-object-safe traits used statically |
| `concat.elx` | 7 | `++` on `str`, `String`, and `Vec`: result types, left-before-right evaluation, chained order, operand independence, self-concatenation, additive precedence against equality and relational operators, empty and nested operands |
| `control.elx` | 7 | left-to-right evaluation of arguments and operands, short-circuiting, all fourteen precedence levels, compound-assignment destination evaluated once, source-order arms, guards, alternatives, every pattern shape, explicit dereference for reference matching, nested loop control |
| `text.elx` | 4.1, 7.2 | `str`/`String` materialization and independence, every escape plus direct Unicode, interpolation order, `{{`/`}}`, user `Display` through `Formatter`, nested and dynamically dispatched `Display` |
| `errors.elx` | 8 | `?` with an exact error type and single operand evaluation, explicit conversion of a foreign error, reverse and inner-first cleanup order, `defer:` blocks, execution-time argument values, cleanup on all five exit edges, a returned handle closed by its own `defer` |
| `traps.elx` | 8.1 | 22 `expect` cases covering every `BuiltinTrap` variant reachable from safe or unsafe source, plus a user-defined `RuntimeTrap` identity and ordinary assertions |

## Findings

Every compiler defect from this audit is resolved and regression-tested.
Findings 1, 2, 4–12, and 15 cover enum-pattern reachability, primitive trait
implementations, shift typing, module `self` paths, transparent collection
aliases, raw-pointer named fields, unit and trait-object equality, fieldless
structs, zero-length arrays, static invalid arithmetic, and variadic placement.

Finding 15 (`non_final_variadic.elx`) is the most recent: a misplaced or
repeated variadic parameter is now rejected by the parser, on ordinary
functions and compile-time declarations alike. The shipped source witnesses the
accepted final form; the rejected forms are asserted in `tests/parser.rs`.

The two language questions found by the audit are settled:

| # | File | Decision |
| --- | --- | --- |
| 3 | `variadic_iteration.elx` | Variadic tails are managed immutable slices with `len`, checked indexing, and copy-yielding iteration. |
| 14 | `callable_erasure_of_function_reference.elx` | Named function references satisfy static `Callable` bounds but do not erase directly to `&Callable`; erasure requires referenced nominal storage. |

Finding 13 (`int128_support.elx`) is a documented implementation limitation,
not a compiler defect: `docs/toolchain.md` records that native `i128`/`u128`
lowering remains unavailable.

## Compile-time surface

`docs/SPEC.md` §12 and the compile-time half of `++` live in the sibling
[`adversarial_macros`](../adversarial_macros) package. The surface and
bounded interpreter are stable; the package is checked rather than run because
it is a compile-time fixture. Its resolved findings remain listed there as
regression evidence.

## Behaviors confirmed correct

Worth recording, because these are the subtle rules the suite was built to
break and did not:

- an interior reference observes replacement of its whole container, and
  mutation through it is visible in that container (SPEC 3.2);
- a returned resource handle is closed by its own unconditional `defer`,
  because the return value is copied before cleanup begins (SPEC 8);
- `?` propagation runs cleanup for every exited scope, in reverse
  registration order, inner block first;
- a deferred call reads the values its expressions hold at execution time, not
  at registration time;
- captures are constructed exactly once, left to right, at the closure
  expression — later rebinding of a plainly captured source does not change
  the snapshot, while a `&var` capture keeps identity across closure copies;
- iteration is a snapshot: growing the source vector inside its own `for` loop
  does not extend that loop;
- every arithmetic, index, key, conversion, and null-pointer trap raises the
  exact `BuiltinTrap` identity `docs/SPEC.md` 8.1 assigns it, and a user-defined
  `RuntimeTrap` keeps its own nominal identity;
- a duplicate literal map key replaces, a duplicate set element collapses, and
  both report the displaced value or membership transition;
- `++` rejects every operand shape outside its contract — numeric operands,
  mixed `str`/`String`, `Set`, `Map`, fixed arrays, `bool`, `a ++= b`, and both
  `++a` and `a++` — while `str ++ str` stays `str` and `String ++ String` stays
  `String`;
- `check`/`build` correctly reject a statically known out-of-bounds array
  index, a chained comparison, a reference to a non-addressable expression, a
  duplicate module item, an alternative pattern binding different names, and a
  non-addressable receiver for a `&Self` method.

Three rejections in this suite were the compiler being right and the first
draft being wrong: `root` is a reserved path keyword, `&1` is not addressable,
and a generic parameter appearing in no parameter position needs an explicit
argument list.

## Notes for extending this suite

- A nested string literal inside an f-string interpolation does not lex
  (`f"{x == \"y\"}"`); hoist the literal to a binding first.
- Keep map and set observations order-independent.
- Do not print a NaN: its sign is left to the C library, which prints `-nan`
  for `0.0 / 0.0` here. Compare instead.
- `regressions/` sits outside `src/`, so its files are not discovered as
  package modules and never affect the build.
