# Language Design Proposals

> Status: Exploratory design discussion. This document is not part of the
> normative language specification and does not authorize grammar or compiler
> changes by itself.
>
> Scope: Foreign declarations, native build configuration, collection literal
> delimiters, and a possible future user-macro system.
>
> M17 resolution: foreign declarations now use
> `@importc("c_name", "header.h")` and `@exportc("c_name")`; there is no
> `extern` grammar. `*fn` and `*unsafe fn` are general raw function pointers,
> not C-only types. The alternatives retained below are historical design
> context rather than open M17 choices.

This document combines the motivating ideas for these features with the
implementation, safety, tooling, and language-design considerations they
introduce. Accepted decisions should eventually move into `SPEC.md`, their
implementation work into `ROADMAP.md`, and unresolved decisions into `ISSUES.md`.

## 1. Attribute-based foreign declarations

### Motivation

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

### Tentative direction

Reserve item attributes now and permit a small compiler-defined set for FFI.
The accepted design uses distinct `@importc` and `@exportc` forms, removes
`extern` entirely, uses general raw function-pointer types at C boundaries,
and keeps user-defined attribute macros out of the M17 dependency chain.

## 2. Top-level `build.elx`

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

The first FFI implementation does not need all of that machinery. Its immediate
requirements can remain declarative:

```toml
[native]
libraries = ["example"]
library_paths = ["native/lib"]
include_paths = ["native/include"]
headers = ["example.h"]
defines = ["EXAMPLE_FEATURE=1"]
link_options = ["-pthread"]
```

The precise manifest keys are illustrative. Static native inputs should be
resolved relative to the package, normalized, ordered deterministically, and
included in build cache keys.

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

### Tentative direction

Design the native build metadata before completing M17 linkage, but implement
the static manifest form first. Reconsider executable `build.elx` after the
standard library, host/target model, and reproducible build protocol exist.

## 3. `@vec` delimiter consistency

### Proposed change

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

### Tentative direction

The existing `@vec[...]` form has the stronger semantic delimiter convention
and is the current preference. If uniform `@name{...}` invocation is chosen as
a foundational rule for the future macro system, change it before the language
surface stabilizes rather than supporting both spellings.

## 4. User-defined macros, derives, and attributes

### Long-term goal

A future metaprogramming system could support:

- ordinary syntax-generating macros;
- user-defined derive generators;
- declaration attributes such as the proposed FFI attributes;
- attribute macros that inspect or transform functions and type declarations.

These facilities could share token-tree parsing and expansion infrastructure,
similar to the broad architecture used by Rust. They should not be treated as
one semantic feature merely because their invocation syntax is related.

### Three distinct expansion roles

An ordinary macro transforms an invocation into an expression, statement,
pattern, type, or item. A derive generator inspects a declared type and emits
one or more implementations. An attribute macro may inspect, replace, remove,
or multiply the declaration to which it is attached.

Each role has different ordering and semantic requirements:

- ordinary macros generally expand before type checking;
- derives need the declared type's fields, generics, and requested trait;
- item attributes may affect which names exist before resolution;
- generated implementations must still obey coherence and orphan rules.

A user trait is not automatically derivable from its method signatures. The
trait does not describe how a method should be synthesized from a type's
fields. A trait whose methods all have defaults already supports an explicit
empty implementation:

```elx
impl MyTrait for MyStruct:
    pass
```

A genuinely custom derive therefore needs an associated generator or a much
more restricted structural-trait mechanism.

### Required design decisions

Before user-defined expansion is introduced, the language needs explicit rules
for:

- macro declaration, import, visibility, and namespace lookup;
- token trees in an indentation-sensitive grammar;
- hygienic generated identifiers;
- expansion order and fixed-point behavior;
- recursion, time, memory, and output-size limits;
- source spans and diagnostics through generated syntax;
- deterministic builds and cache keys;
- whether macro code is interpreted, compiled for the host, or loaded as a
  native plugin;
- dependency trust, filesystem access, process access, and sandboxing;
- generic bounds and name resolution in generated implementations;
- language-server behavior for incomplete and expanded source.

Procedural item attributes are especially expensive for editor tooling because
resolution, completion, navigation, and rename may depend on declarations that
exist only after expansion.

### Staged approach

1. Reserve item-attribute syntax such as `@name(...)`.
2. Implement only compiler-defined metadata attributes, including any accepted
   FFI attributes. These are validated directly and do not execute user code.
3. Retain the existing compiler-supported derives.
4. Design hygienic declarative macros as an independent post-conformance
   feature.
5. Add custom derive generators after macro expansion has stable syntax,
   hygiene, spans, and deterministic limits.
6. Add arbitrary procedural attribute macros only after Elamite has an explicit
   host-side compile-time execution and security model.

The exact `@macro{...}` definition and invocation syntax remains open. Reserving
the `@name` namespace leaves room for that work without making the initial FFI
attributes depend on it.

## 5. Proposed sequencing

These proposals interact, but they do not need to ship as one feature:

1. Review and settle the compiler-defined item-attribute grammar.
2. M17 chose `@importc`/`@exportc` item attributes and removed `extern`.
3. Define declarative include paths, headers, library paths, libraries, and
   link options before implementing native linkage.
4. Resolve the `@vec` delimiter before committing to a general macro invocation
   convention.
5. Complete initial conformance and language-server-friendly semantic
   boundaries.
6. Open a separate design effort for hygienic declarative macros, custom
   derives, procedural attributes, and programmable `build.elx`.

This ordering allows the FFI and build system to progress without prematurely
committing Elamite to arbitrary compile-time code execution.
