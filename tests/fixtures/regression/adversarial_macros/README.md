# Compile-time surface stress package

Companion to [`adversarial`](../adversarial), covering the
compile-time syntax-generation surface from `docs/spec.md` §12.

This package is **checked, not run** because it is a compile-time surface
fixture rather than an executable. It exercises the bounded interpreter,
declaration grammar, the three namespaces and their imports, visibility across
a package boundary, all twelve `quote:` roles, both interpolation forms,
attributes, derives, and function-like macro execution.

```sh
cargo run -- check tests/fixtures/regression/adversarial_macros
cargo run -- dump expanded tests/fixtures/regression/adversarial_macros
cargo run -- fmt --check tests/fixtures/regression/adversarial_macros
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
| `regressions/` | four reproductions, described below |

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
- user forms are stable, while `@vec`/`@map`/`@set`, `@importc`, and
  `@exportc` retain their compatible built-in behavior;
- the formatter round-trips every construct here without changing meaning.

## Findings

| # | File | Status | Summary |
| --- | --- | --- | --- |
| M1 | `macro_invocation_form.elx` | resolved | `@path(...)` is role-neutral at parse time, resolves in the macro namespace, and expands in expression, pattern, type, statement, and item positions. |
| M2 | `attribute_attachment_form.elx` | resolved | `@attr(tag)` resolves and executes structural attributes, including replacement, removal, sibling output, and interacting transforms. |
| M3 | `attached_derive_form.elx` | resolved | `@derive(...)` runs after attributes for user derives and compiler-supported derives; the compact built-in form remains compatibility syntax. |
| M4 | `signature_validation.elx` | resolved | Compile-time signatures reject runtime-only types, invalid variadic placement, unsafe capabilities, and invalid derive/attribute contracts before execution. |

All four findings are retained as regression fixtures. The bounded interpreter,
fixed-point scheduler, hygiene/provenance handling, deterministic limits, and
ordinary semantic re-entry are covered in `tests/expansion.rs` and
`tests/expansion_robustness.rs`.

One finding from this round landed in the sibling package instead, because it
is not macro-specific:
[`non_final_variadic.elx`](../adversarial/regressions/non_final_variadic.elx)
— a misplaced or repeated variadic parameter on an *ordinary* function is
accepted, silently loses its variadic marker in the checker, and reaches the C
compiler as a type-incorrect call. SPEC 12.1 restates the same placement rule
for compile-time declarations, so both surfaces need the one fix.
