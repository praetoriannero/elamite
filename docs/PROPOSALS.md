# Resolved and deferred language design proposals

> Status: Non-normative design archive. This document does not authorize
> grammar or compiler changes; `SPEC.md` owns accepted behavior, `ROADMAP.md`
> owns implementation work, and `ISSUES.md` owns active reviews.

This archive records the rationale behind four earlier proposals. Foreign
attributes, declarative native configuration, collection delimiters, and the
interpreter-backed compile-time system have all been settled. Programmable
`build.elx` remains deferred and is not an active design review.

## 1. Attribute-based foreign declarations (resolved)

### Historical motivation

The earlier specification grouped declarations inside a module-level
`extern "C":` block:

```elx
extern "C":
    struct MyStruct:
        value: u32

    fn c_func(value: *MyStruct) -> *CVoid
```

An attribute-oriented form could place the foreign name and originating header
directly beside each declaration:

```elx
@extern("my_struct_t", "some_header.h")
struct MyStruct:
    value: u32

@extern("my_func", "other_header.h")
fn c_func(x: *u32, value: *MyStruct) -> *CVoid
```

This is attractive because it:

- associates each Elamite declaration explicitly with its C spelling;
- makes headers available to generated C without separate bookkeeping;
- permits foreign and ordinary declarations to coexist in one module;
- establishes an item-attribute syntax that other compiler features could
  reuse later.

### Import and export direction

One `@extern` spelling would be ambiguous if it meant an import for a bodyless
function and an export for a function with a body. Imported declarations and
exported definitions have different safety and code-generation behavior, so a
clearer provisional vocabulary is:

```elx
@importc("my_struct_t", "some_header.h")
struct MyStruct:
    value: u32

@importc("my_func", "other_header.h")
fn c_func(x: *u32, value: *MyStruct) -> *CVoid

@exportc("my_callback")
fn callback(context: *var u32) -> i32:
    return 0
```

Under this model:

- an imported function is bodyless and always unsafe to call;
- an exported function has an Elamite body and an unmangled C entry point;
- the attribute implies C ABI validation for that item;
- callback types use the language's general `*fn(...) -> ...` and
  `*unsafe fn(...) -> ...` raw function-pointer forms.

The accepted names are `@importc` and `@exportc`.

### Headers and C type names

Including the authoritative C header can be safer than emitting a second,
potentially incompatible declaration. It also raises several questions:

- C distinguishes typedef names such as `my_struct_t` from tags such as
  `struct my_struct`.
- Local quoted headers and system angle-bracket headers may need distinct
  treatment.
- Header search paths, defines, and platform selection belong to package build
  configuration.
- If a header supplies a complete type, generated C should avoid redefining
  that type.
- Elamite still needs a field declaration for type checking, so a disagreement
  between the Elamite view and the header must remain an explicit unsafe FFI
  contract or be checked by a generated C harness.

A header could alternatively be attached once to an FFI block or module while
per-item attributes provide only C names:

```elx
@c_header("some_header.h")
extern "C":
    @c_name("my_struct_t")
    struct MyStruct:
        value: u32

    @c_name("my_func")
    fn c_func(value: *MyStruct) -> *CVoid
```

This hybrid retains the visually explicit foreign boundary and avoids repeating
one header on every declaration. The fully attribute-based form is more
flexible. A design review should choose one rather than supporting overlapping
forms initially.

### Restrictions remain semantic

Changing the surface syntax must not turn an arbitrary ordinary declaration
into an ABI-safe declaration without validation. Foreign declarations still
need the restrictions already described by the specification:

- foreign structs cannot be generic, derive traits, or contain methods;
- fields and function signatures must be recursively ABI-safe;
- imported functions cannot have bodies or Elamite variadics;
- calls to imported functions require `unsafe:`;
- ordinary Elamite representations are not implicitly marshalled;
- imported opaque types remain usable only behind raw pointers.

Foreign enums should be designed separately. C enum representation is not
portable enough for an attribute alone to make an ordinary Elamite enum
ABI-safe.

### Resolution

The shipped design uses distinct `@importc` and `@exportc` forms, has no
`extern` grammar, and uses general raw function-pointer types at C boundaries.
User-defined attributes now exist independently through the stable compile-time
system in `SPEC.md` §12.

## 2. Top-level `build.elx` (deferred)

### Motivation

A top-level `build.elx` could provide one place to connect an Elamite package
with:

- system libraries;
- library and header search paths;
- C headers and preprocessor definitions;
- generated native or Elamite sources;
- additional native compilation inputs;
- platform-specific build choices.

This would be more expressive than a fixed manifest and would resemble the
programmable build facilities used by other systems languages.

Project source modules should not normally need to be listed manually.
Elamite's package model already discovers `.elx` files under the package source
directory and derives their module paths deterministically.

### Declarative configuration first

Running `build.elx` as ordinary Elamite code would introduce another execution
stage before package compilation. That requires answers for:

- whether the script is compiled for the host while the package targets
  another architecture;
- which standard-library facilities are available during bootstrapping;
- filesystem, process, environment, and network permissions;
- deterministic outputs and cache invalidation;
- dependency build-script trust and sandboxing;
- generated-file locations and `rerun-if-changed` behavior;
- how build-script errors become source diagnostics.

The implemented FFI support does not need that machinery. Its native inputs
are declared in the package manifest:

```toml
[native]
libraries = ["example"]
library_paths = ["native/lib"]
include_paths = ["native/include"]
link_options = ["-pthread"]
```

The implemented keys are `include_paths`, `library_paths`, `libraries`, and
`link_options`. Static native inputs are resolved relative to the package and
passed deterministically to the selected C toolchain.

### A later programmable build API

If declarative configuration proves insufficient, `build.elx` could later be a
host-side program using a deliberately limited build API:

```elx
fn main(build: &var Build) -> Result[(), BuildError]:
    build.include_path("native/include")
    build.link_library("example")
    build.rerun_if_changed("native/example.c")
    return Result.Ok(())
```

This should produce build metadata rather than mutate compiler internals. It
should write generated artifacts only beneath a designated output directory
and explicitly report every input that affects rerunning or caching.

### Current status

Static manifest configuration is implemented. Executable `build.elx` remains
deferred until a concrete need justifies its host/target, capability, caching,
and reproducibility contract.

## 3. `@vec` delimiter consistency (resolved)

### Considered change

Change vector construction from:

```elx
@vec[1, 2, 3]
```

to:

```elx
@vec{1, 2, 3}
```

This would make all three compiler-provided collection forms use braces:

```elx
@vec{1, 2, 3}
@map{"one": 1}
@set{"one"}
```

The macro name already determines the resulting collection, so the shared
delimiter is grammatically unambiguous.

### Competing consistency

The existing spelling follows a different but meaningful convention:

```elx
[1, 2, 3]       // fixed array
@vec[1, 2, 3]   // growable ordered sequence
@map{"one": 1}  // key-value collection
@set{"one"}     // unordered unique collection
```

Under that interpretation, brackets denote ordered sequences and braces denote
associative or uniqueness-based collections. It also visually relates a
`Vec[T]` to ordinary array indexing.

The choice is therefore between macro-family consistency and collection-shape
consistency. It has no significant semantic or implementation consequence, but
it is a source-breaking grammar change that would touch the specification,
demonstration, parser snapshots, and collection tests.

### Resolution

The language retains `@vec[...]`, matching the ordered-sequence convention;
`@map{...}` and `@set{...}` retain braces. User-defined function-like macros
use `@path(...)`, so there is no uniform `@name{...}` rule requiring a change.

## 4. User-defined macros, derives, and attributes (resolved)

The accepted system has three module-level declaration forms with separate
namespaces: `[pub] macro`, `[pub] attr`, and `[pub] derive`. Their ordinary safe
Elamite bodies execute in a bounded, capability-free compile-time interpreter
over immutable `std.ast` 1.0 values.

- Function-like macros are invoked as `@path(...)` and may produce expressions,
  statements, patterns, types, members, items, or their documented lists.
- Attributes attach as `@attr(path(...))` and may transform, replace, remove,
  or multiply supported definitions.
- Derives attach as `@derive(path)` and transform a single target while
  producing the requested implementation structure.
- `quote:` constructs typed syntax; `$name` and `$(expression)` interpolate AST
  values, while `++` concatenates supported strings, sequences, and AST lists.
- Expansion runs attributes before derives and then uses a deterministic
  fixed-point scheduler with bounded depth, executions, generated nodes, fuel,
  and live values.
- Generated identifiers use definition-site context, interpolated syntax keeps
  its existing context, and provenance retains physical invocation and
  definition evidence without fabricating source spans.

This replaced the earlier matcher/transcriber and native-plugin possibilities.
The former `--unstable-macros` gate has been retired; user-defined forms are
stable and covered by the macro example, adversarial package, property tests,
and the conformance ledger. `SPEC.md` §12 is authoritative.

## 5. Current disposition

| Proposal | Current state |
| --- | --- |
| Foreign declaration attributes | Implemented as `@importc` and `@exportc` |
| Declarative native configuration | Implemented in `[native]` manifest fields |
| Programmable `build.elx` | Deferred; no active review or accepted surface |
| Vector delimiter change | Rejected; `@vec[...]` remains canonical |
| User macros, attributes, and derives | Implemented and stable under `SPEC.md` §12 |
