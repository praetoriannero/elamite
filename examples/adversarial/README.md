# Adversarial conformance package

A single executable package that pushes the implemented parts of Elamite to
their specified boundaries, plus a directory of minimal reproductions for the
places where `SPEC.md` and the compiler currently disagree.

Two halves:

- **`src/`** — a package that builds, runs, and passes its own tests today.
  Every observation it prints is a rule from `SPEC.md`. If a change to the
  compiler alters any line of its output, a specified behavior moved.
- **`known_failures/`** — one standalone `.elx` file per divergence, each
  carrying the normative quotation, the expected behavior, and the exact
  current behavior. None of them compile and run today; each is a ready-made
  regression test for whoever fixes it.

## Running it

```sh
cargo run -- run examples/adversarial            # ~200 observations
cargo run -- test examples/adversarial           # 26 trap and assertion tests
cargo run -- run examples/adversarial --release
cargo run -- run examples/adversarial --target=x86
```

One line of output is target-dependent by design: `usize bit width` reports 64
on x86-64 and 32 on x86 (`SPEC.md` 4.1). Map and set iteration order is
unspecified, so nothing observes it — only order-independent aggregates.

Reproduce a divergence with:

```sh
cargo run -- build examples/adversarial/known_failures/variant_literal_arm.elx
```

## What `src/` covers

| Module | `SPEC.md` | Boundary pushed |
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
| `control.elx` | 7 | left-to-right evaluation of arguments and operands, short-circuiting, all fourteen precedence levels, compound-assignment destination evaluated once, source-order arms, guards, alternatives, every pattern shape, explicit dereference for reference matching, nested loop control |
| `text.elx` | 4.1, 7.2 | `str`/`String` materialization and independence, every escape plus direct Unicode, interpolation order, `{{`/`}}`, user `Display` through `Formatter`, nested and dynamically dispatched `Display` |
| `errors.elx` | 8 | `?` with an exact error type and single operand evaluation, explicit conversion of a foreign error, reverse and inner-first cleanup order, `defer:` blocks, execution-time argument values, cleanup on all five exit edges, a returned handle closed by its own `defer` |
| `traps.elx` | 8.1 | 22 `expect` cases covering every `BuiltinTrap` variant reachable from safe or unsafe source, plus a user-defined `RuntimeTrap` identity and ordinary assertions |

## Findings

Fourteen places where the implementation and the documents disagree. Ordered
by severity. Numbers match the `known_failures/` files.

### Silently accepted, then broken

These pass `elamc check` with exit 0 and no diagnostic, then fail. A clean
`check` that cannot build is itself a defect: `LEDGER.md` 14 defines `check` as
"resolution + type checking without generating C or linking; report
diagnostics."

| # | File | Rule | Behavior |
| --- | --- | --- | --- |
| 7 | `raw_pointer_field_access.elx` | `AGENTS.md` closures section (done): `pointer.value` auto-dereferences inside `unsafe:` | lowering rejects it as outside the "Milestone 8 executable subset". The tuple form `pointer.0` is fully implemented, including assignment through `*var` |
| 6 | `alias_collection.elx` | SPEC 4.4 "A module-level `type` alias is transparent" | a collection method on an alias-annotated binding fails lowering; separately, routing a collection through an alias emits an unused `el_map_new_tN`, which the driver's own `-Werror` build rejects |
| 8 | `unit_equality.elx` | SPEC 4.5 "Integral primitives, `bool`, `char`, unit, and `str` provide total equality" | emits `==` on the `el_unit` struct: `invalid operands to binary ==` |
| 9 | `trait_object_equality.elx` | SPEC 4.5 "Trait-object references likewise compare their concrete target identity" | emits `==` on the fat-reference struct. `LEDGER.md` 4.5 records this row as run-pass tested |
| 10 | `field_less_struct.elx` | SPEC 2.2 `pass` as the explicit empty body; SPEC 4.2 sets no minimum field count | the generated copy helper never reads its parameter: `unused parameter 'value' [-Werror=unused-parameter]`. Declaring and constructing is enough |
| 11 | `zero_length_array.elx` | SPEC 4.1 `[T; N]` where `N` is "nonnegative" | two errors: `excess elements in array initializer` and `comparison of unsigned expression in '< 0' is always false` |
| 13 | `int128_support.elx` | SPEC 4.1 lists `i128`/`u128` with no caveat | diagnosed clearly rather than miscompiled, so this is a documentation gap: `ROADMAP.md` 2.5 requires temporary limitations to be "recorded explicitly", and no top-level document records this one |

Findings 8 and 9 share one root cause — the backend emits C `==` directly on a
struct type instead of a comparison helper — so both scalar-izable aggregate
comparisons should be fixed together.

### Valid programs rejected

| # | File | Rule | Behavior |
| --- | --- | --- | --- |
| 1 | `variant_literal_arm.elx` | SPEC 7 "A statically unreachable arm is a compile-time error" | **highest severity.** Any literal sub-pattern inside an enum variant marks every later arm on that variant unreachable. `Option.Some(0)` followed by `Option.Some(other)` does not compile. Reproduces for tuple-like and record-like variants; plain struct patterns and guards are unaffected |
| 2 | `trait_impl_for_primitive.elx` | SPEC 6 orphan rule permits it when the trait is local | `impl Describe for i32` is accepted with no diagnostic, then every use fails: bound and qualified calls fail lowering, and a generic bound reports "does not satisfy required `Describe` capability". The same trait on a struct works |
| 5 | `self_expression_path.elx` | SPEC 2.3 "`self` begins at the current module" | `use self.inner.value` resolves, but `self.local()` in an expression path reports "cannot resolve `self`". `super` works in both positions |

### Specification contradicts itself or the implementation

| # | File | Rule | Behavior |
| --- | --- | --- | --- |
| 3 | `variadic_iteration.elx` | SPEC 5's own normative example iterates a variadic tail with `for` | SPEC 7.1 restricts `for` to arrays, `Vec`, `Map`, and `Set`, which excludes the slice `[T]` that SPEC 5 binds. The example in the specification does not compile. Indexing works; `rest.len()` also fails, leaving no portable way to bound the index |
| 4 | `shift_operand_typing.elx` | SPEC 7 "A shift count must have an unsigned integer type" | the implementation instead requires both operands to share one concrete type, so `1i32 << 3u32` is rejected and `1i32 << 3i32` — a signed count — is accepted. Wrong in both directions. The runtime width check is correct |
| 12 | `static_arithmetic_evidence.elx` | SPEC 4.1 "A statically evident invalid literal, conversion, or arithmetic operation is a compile-time error" | `2147483647 + 1`, `1i32 << 32i32`, and `1i32 / 0i32` all compile and trap at run time. The array half of the same rule *is* implemented. `LEDGER.md` 4.1 assigns static detection to M6 and marks the row complete |

### Ambiguity worth settling

| # | File | Question |
| --- | --- | --- |
| 14 | `callable_erasure_of_function_reference.elx` | `LEDGER.md` 5 says "named safe function references also satisfy matching callable APIs; `&Callable` provides erasure". A `Callable` bound does accept a named function reference; erasing that reference behind `&Callable` does not. SPEC 6 arguably justifies the rejection, since the target of an `&fn` is a function rather than a type implementing `Callable`. SPEC 5.1 should say so explicitly |

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
  exact `BuiltinTrap` identity `SPEC.md` 8.1 assigns it, and a user-defined
  `RuntimeTrap` keeps its own nominal identity;
- a duplicate literal map key replaces, a duplicate set element collapses, and
  both report the displaced value or membership transition;
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
- `known_failures/` sits outside `src/`, so its files are not discovered as
  package modules and never affect the build.
