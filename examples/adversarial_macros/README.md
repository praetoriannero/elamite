# Compile-time surface stress package

Companion to [`examples/adversarial`](../adversarial), covering the
compile-time syntax-generation surface from `SPEC.md` §12.

This package is **checked, not run**. `ROADMAP.md` places *Compile-time
checking and lowering* and *Bounded interpreter* after the just-completed
`std.ast` façade, quotation syntax, and `++` work, so no compile-time
declaration executes yet. Everything here exercises the surface that does
exist: declaration grammar, the three namespaces and their imports, visibility
across a package boundary, all twelve `quote:` roles, both interpolation
forms, and the `--unstable-macros` gate.

```sh
cargo run -- check examples/adversarial_macros --unstable-macros
cargo run -- check examples/adversarial_macros            # 40 gate errors
cargo run -- fmt --check examples/adversarial_macros
```

`provider/` is a dependency package supplying public compile-time declarations,
a package-private one, and a re-export, so cross-package visibility is tested
rather than assumed.

## Layout

| File | Covers |
| --- | --- |
| `src/quoting.elx` | all twelve `quote:` roles — `Expression`, `Pattern`, `TypeSyntax`, `StatementList`, `MemberList`, `Item`, `ItemList`, `StructDefinition`, `EnumDefinition`, `FunctionDefinition`, `Implementation`, `FieldDefinition` — plus `$name` and `$(expression)` interpolation, nested quotes, role inference from a return position, a variadic tail, deep indentation inside a quoted body, and `++` joining a `MemberList` |
| `src/namespaces.elx` | one spelling bound in four namespaces at once, `use macro`/`use attr`/`use derive`, aliases, re-exports, `self` path roots, package-private access inside the package, and imports across a package boundary |
| `provider/src/lib.elx` | the dependency side: public and package-private compile-time declarations and a public re-export |
| `known_failures/` | four reproductions, described below |

Each stress module ends with a documentation comment listing the forms that are
correctly **rejected**, so the negative surface is recorded even though a
rejected form cannot live in a checking package.

## What holds up

The implemented surface is in good shape. Everything below was probed
adversarially and behaved correctly:

- all twelve quote roles parse, and a body in the wrong role is rejected
  against the ordinary grammar (`expected an expression`, `expected a
  declaration`);
- a quote with no annotation and no return-position role is rejected with
  "this quote has no expected `std.ast` role"; an annotation naming a
  non-`std.ast` type is rejected separately;
- `quote:` in an ordinary runtime function is rejected, as is `$` outside a
  quote body, `$` with no identifier or `(`, and `$()` with no expression;
- the three compile-time namespaces are genuinely separate — one module can
  declare a macro, an attribute, a derive, and an ordinary function that all
  share a name — while a duplicate *within* one namespace is diagnosed;
- cross-package visibility is enforced: a public compile-time declaration
  imports, a package-private one reports "macro `name` is package-private",
  and `pub use macro` of a package-private declaration is rejected with a
  related span pointing at the offending declaration;
- the gate covers declarations, imports, attachments, and quotation, and the
  compiler's own `@vec`/`@map`/`@set`, `@importc`, and `@exportc` stay ungated;
- the formatter round-trips every construct here without changing meaning.

## Findings

| # | File | Status | Summary |
| --- | --- | --- | --- |
| M1 | `macro_invocation_form.elx` | **defect** | `@path(...)`, the only function-like invocation syntax SPEC 12.4 defines, cannot be parsed. `recover_macro_invocation` in `src/parser.rs` accepts only `[` or `{`, so the form has no representation. Two knock-on effects: with `--unstable-macros` *on* and the macro declared and collected, the message is "unknown compiler macro `@make`; expected `@vec`, `@map`, or `@set`"; and the `MacroExpression` branch in `src/expansion/gate.rs` that produces "user-defined macro invocations require `--unstable-macros`" is unreachable for that form |
| M2 | `attribute_attachment_form.elx` | on plan, bad diagnostic | `@attr(tag)` reports "unknown compiler attribute `@tag`". The wrapper unwraps its argument and looks it up among `@importc`/`@exportc`, never consulting the attribute namespace `src/expansion/namespace.rs` already populates |
| M3 | `attached_derive_form.elx` | on plan, bad diagnostic | `@derive(Default)` reports "unknown compiler attribute `@Default`". SPEC 4.3 calls the attached spelling the *general* derivation form and the compact `struct Point(Default):` the compatibility form, but only the compact one works |
| M4 | `signature_validation.elx` | on plan | a checklist, not a defect report. Six signatures violating stated SPEC 12.1 rules are accepted today — runtime return and parameter types, reference and raw-pointer parameters, a misplaced variadic, and a two-parameter derive — along with unchecked bodies. Each should be rejected once *Compile-time checking and lowering* lands |

M2, M3, and M4 are scheduled work, not regressions; they are recorded so the
pending milestone has concrete cases and so the diagnostic-quality issues in
M2 and M3 are not lost. M1 is the one that needs a decision now, because the
grammar cannot express the specified syntax at all.

One finding from this round landed in the sibling package instead, because it
is not macro-specific:
[`non_final_variadic.elx`](../adversarial/known_failures/non_final_variadic.elx)
— a misplaced or repeated variadic parameter on an *ordinary* function is
accepted, silently loses its variadic marker in the checker, and reaches the C
compiler as a type-incorrect call. SPEC 12.1 restates the same placement rule
for compile-time declarations, so both surfaces need the one fix.
