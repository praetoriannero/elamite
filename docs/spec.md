# Elamite Language Specification

> Status: Draft
>
> Version: 0.10.0-draft
>
> This document is the normative 0.10 design. The compiler has implemented
> shallow ordinary-copy lowering but retains transitional 0.9 collection,
> concurrency, and pointer behavior until the ordered migration in
> [roadmap.md](roadmap.md) completes. Its version identity and current
> [specification demonstration](../examples/spec_demo.elx) remain 0.9 rather
> than claiming partial conformance. Ambiguities and internal inconsistencies
> that still need decisions are listed in [issues.md](issues.md).

## 1. Overview

Elamite is a statically typed, garbage-collected language that compiles to C.
It provides value types, explicit references, traits, generic types, algebraic
data types, recoverable errors, raw pointers behind an unsafe boundary, and
indentation-delimited control flow.

Ordinary values are passed and assigned by shallow value copy. Inline scalar
and aggregate storage is copied, while references, pointers, functions,
collection descriptors, mutable text backing, trait-object references, and
resource handles preserve the identities they contain. Passing
`&value` explicitly passes a shared reference; passing `&var value` explicitly
passes a mutable reference. Elamite has no source lifetime parameters. Managed
memory uses Boehm GC;
programs should not use collection timing for resource cleanup.

## 2. Program layout

Elamite source files are UTF-8 text. An identifier begins with an ASCII letter
or `_` and continues with ASCII letters, decimal digits, or `_`; equivalently,
it matches `[A-Za-z_][A-Za-z0-9_]*`. Keywords are reserved and cannot be used
as identifiers. The reserved keywords are `as`, `attr`, `break`, `continue`,
`defer`, `derive`, `else`, `enum`, `expect`, `false`, `fn`, `for`, `if`,
`impl`, `in`, `let`, `macro`, `match`, `mod`, `null`, `pass`, `pub`, `quote`,
`return`, `root`, `self`, `Self`, `struct`, `super`, `test`, `trait`, `true`,
`type`, `unsafe`, `use`, `var`, and `while`.
Unicode text remains valid in comments, documentation, and string or character
contents as permitted by their literal syntax.

### 2.1 Comments and documentation

`//` introduces a line comment. A multiline comment is written as consecutive
`//` lines. A line beginning `///` is a documentation comment; its contents are
Markdown documentation for the declaration that follows it.

~~~elx
// Ordinary comment
// Second line of a multiline comment

/// # A documented type
/// Documentation supports Markdown.
struct Documented:
    value: String
~~~

### 2.2 Indentation and bodies

Leading indentation uses spaces only; a tab in leading indentation is an error.
Every nested block adds exactly four spaces. Each body-bearing declaration and
control-flow construct uses a trailing `:` followed by a newline and one deeper
indentation level. The body ends at the next dedent. Dedentation must return to
a previously established block indentation level. EOF closes every remaining
open block.

This body form is used for `mod`, `attr`, `derive`, `macro`, `quote`, `struct`,
`enum`, `trait`, `impl`, `test`, `expect`, `if`, `else`, `match`, `for`,
`while`, `unsafe`, and function declarations with bodies. Brace-delimited
bodies and same-line bodies are invalid everywhere. Compile-time declaration
bodies contain ordinary Elamite statements; a `quote:` body contains syntax in
the expected `std.ast` role under Section 12. An empty body is also invalid;
`pass` is the explicit no-op statement when an ordinary body must otherwise be
empty.

Blank lines and ordinary comment-only lines do not affect indentation.
Documentation comments use the indentation of the declaration they document
and do not themselves open or close a block.

Outside `()`, `[]`, and `{}`, a newline normally ends a statement. A statement
may continue on following physical lines indented exactly four spaces beyond
the indentation where that statement began. Those lines form one logical
statement when their combined tokens parse as one statement, even if an earlier
physical line could parse by itself. A body colon takes precedence and opens a
block rather than a continuation. The continuation ends when code returns to
the statement's starting indentation. An otherwise unexpected indentation is
an error.

Inside `()`, `[]`, and `{}`, physical newlines and indentation do not terminate
the surrounding expression or create blocks. Elamite has no backslash line
continuation. Record literals use braces; parentheses form tuples and group
expressions. Parser error recovery is an implementation detail rather than part
of language conformance.

~~~elx
if enabled:
    println("enabled")
else:
    println("disabled")

let point = Point { x: 1.0, y: 2.0 }
let pair = (point.x, point.y)
let total = point.x
    + point.y
~~~

### 2.3 Modules, imports, and visibility

A package is the unit of compilation, dependency resolution, nominal identity,
and trait coherence. Every package has an `elamite.toml` manifest declaring its
name, version, target kind, dependencies, and root source file. Target kind is
either `lib` or `exe`. `src/lib.elx` and `src/main.elx` are the
respective default roots; the manifest may select a different `.elx` file. The
directory containing the selected root file is the package's source directory.

The initial resolver supports local path dependencies only. A dependency entry
uses its manifest key as the dependency alias and supplies a `path` to a directory
containing another `elamite.toml`. The path is resolved relative to the
depending package's manifest directory.

~~~toml
[package]
name = "app"
version = "0.1.0"
target_kind = "exe"

[dependencies.codec]
path = "../codec"
~~~

Formatting preferences are optional tooling metadata rather than part of
package identity or language semantics. `[format].line_length` is a positive
integer specifying the formatter's preferred maximum line length; it defaults
to 100 when omitted. A command-line override takes precedence over this value.

~~~toml
[format]
line_length = 100
~~~

Dependency resolution follows these entries transitively. The initial resolver
does not fetch registry or Git dependencies, select among version constraints,
or read or write a package lockfile. Those facilities may be added by a future
resolver without changing how the compiler consumes the resolved package
graph.

The selected file defines the package's `root` module. Every other `.elx` file
beneath the source directory defines a file-backed module from its relative
path: `models.elx` defines `root.models`, and `codec/json.elx` defines
`root.codec.json`. Directories create namespace components. A file such as
`codec.elx` may define declarations for `root.codec` while files beneath
`codec/` define its children. Source file and directory components must be
valid identifiers.

`mod name:` introduces an indentation-delimited inline nested module. Inline
and file-backed modules cannot define the same module path. File-backed modules
are discovered from the package source directory and require no bodyless `mod`
declaration.

Paths use periods. `root` begins at the current package root, `self` begins at
the current module, and `super` begins at its parent; using `super` in the root
module is an error. These three are keywords because they name a position
relative to the current module rather than a declaration.

`std` names the standard-library package. It is an ordinary name, not a
keyword: it begins a path in the same way a dependency's manifest alias does,
and a module that declares or imports its own `std` shadows the
standard-library package within that module. Each dependency's manifest alias
likewise begins a path into that dependency. An unqualified name is found only
among lexical bindings, declarations and imports in the current module, and
prelude names; lookup never searches unrelated modules.

`use path` is permitted at module level, including within an inline module,
and binds the final path component in that module. `use path as name` uses an
explicit local alias. Imports are not inherited by nested modules. Wildcard and
grouped `use` declarations are initially unsupported. Import order has no
semantic effect.

Declarations are package-private unless prefixed with `pub`: every module in
the defining package may access a package-private declaration, but dependent
packages may not. `pub` applies to modules, functions, structs, enums, traits,
type aliases, macros, attributes, and derives. Fields and inherent methods are
package-private unless individually marked `pub`. All variants and variant
payload fields of a public enum are public. All methods of a public trait are
public as defined in Section 6.

`pub use path` and `pub use path as name` re-export a public declaration
under the bound name. A file-backed module namespace may also be re-exported,
which exposes its public contents beneath that name without exposing its
package-private contents. A public declaration in a file-backed module is not
externally reachable unless the module or declaration is re-exported from an
externally reachable module. Re-exporting changes neither the target's nominal
identity nor its defining package.

A public declaration's externally visible signature may mention only types,
traits, aliases, and bounds that are publicly accessible at that point. A
private field or method is not part of the public signature of its containing
type. Exposing a less-visible item in a public signature is a compile-time
error.

Modules, types, traits, functions, module-level values, aliases, and imports
share one module-item namespace. Defining or importing the same name twice in a
module is an error even when both imports resolve to the same item; an explicit
alias resolves the conflict. Function-local lexical bindings may shadow
module-item names. Macros, attributes, and derives use the separate namespaces
and explicit imports defined in Section 12.1.

Circular imports between modules of one package are permitted. The compiler
collects module declarations before resolving imports and bodies. Imports
execute no code and establish no runtime initialization order. The package
dependency graph must be acyclic.

Under the initial local-path resolver, a package has an opaque identity
determined by its canonical manifest-directory path, not by its displayed name
or version. Two dependency paths that resolve to the same canonical directory
identify one package instance. Different canonical directories identify
different package instances, even when their manifests contain the same name
and version. A dependency alias does not change that identity. The package
identity used by these rules is the package identity used by nominal types,
trait coherence, and the orphan rule in Section 6.

~~~elx
use std.io
use root.models.User as InternalUser
use root.codec.json as json

pub mod diagnostics:
    use std.io
    use super.UserId

    pub fn report(message: String):
        io.println(message)

pub type UserId = u64
pub use root.models.User
pub use root.codec as codec
~~~

## 3. Values, mutability, and references

### 3.1 Copying values

`let` creates a non-rebindable binding that is not itself a mutable place. `var`
creates a rebindable mutable place. A field selected through an ordinary `let`
aggregate cannot be assigned through that path, and that path cannot be used to
form `&var`. This restriction applies through nested ordinary values. It does
not remove mutation capabilities carried by explicit aliases: a non-rebindable
binding whose value is an `&var T`, or whose value contains one, cannot replace
that reference but may still mutate its target.

A local `let` or `var` may bind either one identifier or an irrefutable tuple
binding pattern. Tuple binding patterns may nest and may contain identifiers or
`_`; `()` and `(name,)` are the empty and one-element forms. Literals,
alternatives, enum variants, record patterns, dereference patterns, guards,
rest patterns, and tuple patterns in parameters or loop headers are not local
binding patterns. An optional annotation applies to the complete pattern:
`let (number, name): (i32, String) = value`.

The initializer is evaluated exactly once before any new binding enters scope
and must have the exact tuple shape and arity of the pattern. Every identifier
in one pattern must be unique. All of its bindings enter scope together after
the initializer; `_` creates no binding. Each `let` component receives a
shallow copy and is non-rebindable. Each `var` component receives a shallow
copy in its own rebindable local place. The initializer and any source value
remain usable under the ordinary copy rules.

Assignment, ordinary argument passing, and ordinary returns copy the source
value; using the source after that operation is valid. Copying is a core
property of every value and is not controlled by a trait.

An ordinary copy is shallow and fieldwise. Scalars and inline aggregate slots
are copied into the destination, while every copied descriptor, reference,
pointer, callable, collection handle, and resource handle preserves the backing
or target identity stored in that field. Copying a nested aggregate repeats
this rule for its immediate fields; it does not recursively duplicate reachable
managed backing.

Consequently, rebinding an inline field in one struct copy does not rebind the
corresponding field in another copy, but mutating storage reached through a
copied descriptor is visible through every descriptor that still reaches that
storage. `Vec` uses Go-like pointer/length/capacity descriptor copies, while
`Map` and `Set` copies preserve the identity of their complete mutable table.
The exact built-in behaviors are specified in Section 4.1.

~~~elx
struct User:
    name: String
    tags: Vec[String]

let original = User {
    name: String.from("Ari"),
    tags: @vec[String.from("first")],
}
var changed = original
changed.name = String.from("Bea")
changed.tags[0] = String.from("changed")

println(original.name)    // "Ari": only changed.name was rebound
println(original.tags[0]) // "changed": vector backing is shared

var counter = 0
let alias = &var counter
let copied_alias = alias
*copied_alias = 1
println(*alias) // 1: references retain their explicit aliasing
~~~

### 3.2 References

`&T` is a shared reference to `T`. `&var T` is a mutable reference to `T`.
The expression `&value` forms a shared reference, and `&var value` forms a
mutable reference to a mutable place. Reference field and method access
automatically dereferences the reference.

Reference formation is explicit except for the receiver of a bound method call.
Any other context that expects `&T` or `&var T` never implicitly converts a `T`
place; the source expression must use `&value` or `&var value`.

~~~elx
var point = Point { x: 0.0, y: 0.0 }
let view: &Point = &point
let edit: &var Point = &var point

println(view.x)
edit.x = 1.0
~~~

An `&var T` reference may update fields of its target. A mutable reference
parameter expresses caller-visible mutation. References are not exclusive: any
number of shared and mutable references may name the same storage. Their reads
and writes follow ordinary sequential execution; later writes replace earlier
ones. Elamite performs no borrow or alias checking.

~~~elx
var count = 0
let first: &var i32 = &var count
let second: &var i32 = &var count

*first = 1
*second = *second + 1
println(count) // 2
~~~

Following Go-style addressability, a reference operand must be an addressable
place, such as a binding or a field of an addressable value. Function results
and computed expressions are not addressable, so references to them are
invalid. A composite literal is the explicit exception:
`&Point { x: 0.0, y: 0.0 }` is a referenced composite literal. It creates a
GC-managed target without a separate source-level binding and returns a
reference to that target.

~~~elx
let point: &Point = &Point { x: 0.0, y: 0.0 } // valid
let from_call: &Point = &make_point()         // invalid
let from_sum: &i32 = &(left + right)          // invalid
~~~

Collection interiors are never addressable for safe-reference formation.
Neither shared nor mutable references may be formed to array or `Vec` elements,
`Map` keys or values, or `Set` elements. Collection access in value context
instead returns an ordinary shallow copy.

An array or `Vec` element and a `Map` value may still be an assignable place
when reached through a mutable collection path. Replacement, compound
assignment, and direct nested-field mutation update the backing reached through
that descriptor, but no safe reference to the selected interior may escape.
`Map` keys and `Set` elements are never mutable places; changing one requires
removing it and inserting a new value. Raw-pointer APIs may expose backing
explicitly under their own unsafe contracts.

~~~elx
var points = @vec[Point { x: 0.0, y: 0.0 }]
let original_points = points
var first = points[0]

first.x = 0.5
points[0].x = 1.0

println(first.x)              // 0.5
println(points[0].x)          // 1.0
println(original_points[0].x) // 1.0: both descriptors reach the same backing

// Invalid: collection interiors cannot be referenced.
// let first_ref = &var points[0]
~~~

References are valid struct fields, enum payloads, collection elements,
parameter types, and return types. A reference formed through safe
code remains valid while the reference is reachable. If such a reference to a
local binding or field may escape its scope, the compiler promotes the required
storage to GC-managed storage. Escape analysis may retain nonescaping storage
on the stack as an unobservable optimization.

Returning a reference to a local binding from safe code is therefore valid. A
safe reference to language-managed storage keeps its target reachable through
the garbage collector, including referenced composite literals.

A reference formed directly from a binding points to that binding's storage.
It observes later assignments to the binding. Promotion preserves this behavior
when the reference escapes.

~~~elx
var point = Point { x: 0.0, y: 0.0 }
let view: &Point = &point

point = Point { x: 1.0, y: 1.0 }
println(view.x) // 1.0

fn answer() -> &i32:
    let value = 42
    return &value // valid: `value` is promoted because the reference escapes
~~~

A reference path that enters a nested aggregate points to the storage of that
subvalue within its container. Replacing the container writes through that
storage, so the reference observes the new value. This is the same single rule
as a binding reference: a reference names storage, and every assignment that
overwrites that storage is observable through it.

~~~elx
var user = User {
    name: "Ari",
    address: Address { city: "Aster" },
}
let city: &String = &user.address.city
println(city) // "Aster"

user = User {
    name: "Bea",
    address: Address { city: "Beacon" },
}
println(city) // "Beacon"
~~~

Mutation through a reference into an aggregate is likewise visible in the
container, because both name the same storage.

~~~elx
var located = User {
    name: "Cyd",
    address: Address { city: "Calder" },
}
let relocate: &var String = &var located.address.city

*relocate = "Cove"
println(located.address.city) // "Cove"
~~~

A reference into an aggregate keeps its whole container reachable, not only
the selected subvalue.

### 3.3 Raw pointers and null

Raw pointer types are `*T` and `*var T`. A raw pointer can be `null`; `&T` and
`&var T` are always non-null. A nullable safe reference is represented by
`Option[&T]` or `Option[&var T]`. Conditions require `bool`, so neither raw
pointers nor references have implicit truthiness. Code tests a raw pointer with
an explicit comparison such as `pointer == null`.

Every non-null raw pointer has provenance for one storage instance, a designated
byte extent within that storage, and one position in or one-past that extent.
Converting a safe reference to a raw pointer establishes an extent containing
the referenced target as one element. Foreign code may establish a larger
array extent through its contract. Copying a raw pointer or converting
`*var T` to `*T` preserves its provenance, extent, and position. Reuse of the
same numerical address for a later storage instance does not give an older
pointer provenance for the new instance. `null` has no provenance or storage
position.

Raw data pointers support typed arithmetic in an `unsafe` context. For a
complete, nonzero-sized pointee type, `pointer + offset` and
`pointer - offset` accept `isize` and return the same raw-pointer type;
`var` pointer places additionally support `+=` and `-=`. The offset is measured
in pointee elements. The result must remain within the same array extent or its
one-past position. A single non-array target behaves as an array of length one.
Creating any other position is undefined behavior even when it is not
dereferenced.

Subtracting two non-null pointers with the same resolved pointee type is unsafe
and returns their element distance as `isize`; `*T` and `*var T` may be mixed.
Both operands must identify positions in the same live extent, and the distance
must be representable by `isize`, or behavior is undefined. Pointer arithmetic
is invalid for function pointers, incomplete or zero-sized pointees, and
`std.ffi.CVoid`.

`pointer[index]` accepts an `isize` index and is equivalent to
`*(pointer + index)`, except that the pointer and index are each evaluated
exactly once in left-to-right order. Indexing requires `unsafe`, performs no
bounds check, and produces a read-only raw target for `*T` or an assignable raw
target for `*var T`. The existing prohibition on forming a safe reference to a
raw target remains; conversion of the computed pointer with `as` is explicit.

Equality and inequality remain safe for compatible raw pointers, including
`null`, and compare address identity without requiring common provenance. Raw
data pointers also support `<`, `<=`, `>`, and `>=` as compiler-recognized
unsafe operations without implementing `PartialOrd` or `Ord`. Null is ordered
below every non-null pointer. Ordering two non-null pointers is defined only
when they have the same resolved pointee type and identify positions in the
same live extent, in which case positions later in the extent compare greater;
ordering unrelated non-null pointers is undefined behavior. Mixed `*T` and
`*var T` operands are permitted. The null rule is lowered explicitly rather
than relying on a C relational comparison with a null pointer.

An explicit `as` cast may change a raw pointer's pointee type only in an
`unsafe` context. The cast preserves the address, storage provenance,
designated byte extent, position, and mutability permission; subsequent
arithmetic uses the new pointee size. A cast does not make the storage contain
a valid value of the new pointee type or enlarge its extent. A `*T` therefore
cannot be cast to any `*var U`. Integer-to-pointer and pointer-to-integer
conversions remain unavailable. Foreign code may supply a raw pointer with
provenance, extent, and access rights established by the foreign contract in
Section 10.

`&T` may convert safely to `*T`; `&var T` may convert safely to `*var T`.
`&var T` may also convert to `*T`, and `*var T` may be downgraded to `*T`.
Dereferencing any raw pointer or converting one to a reference requires an
`unsafe` context. Writing through a raw pointer additionally requires
`*var T`; a `*T` target is read-only even in unsafe code. Converting a raw
pointer to a reference asserts that it is non-null, correctly aligned, points
to a valid value of the target type, and remains valid for every use of the
resulting reference. An `unsafe` context does not make an ordinary reference
nullable.

~~~elx
var value = 41
let edit: &var i32 = &var value
let pointer: *var i32 = edit as *var i32

if pointer != null:
    unsafe:
        let recovered: &var i32 = pointer as &var i32
        *recovered = 42
~~~

A raw pointer may be dereferenced or indexed only while its original storage
instance is alive, its current position identifies an initialized pointee
element rather than the one-past position, and the requested access remains
within its designated extent. A write additionally requires writable storage.
The pointer's provenance and these obligations apply even if another storage
instance later occupies the same address. For
language-managed storage, retaining a separate strong language path is part of
the liveness obligation because a raw pointer is not a root. For foreign or
manually managed storage, the foreign contract determines its lifetime and
access rights.

Every executed raw dereference, pointer index, and raw-to-reference conversion
checks for null and correct alignment before accessing the target and traps if
either check fails. Such an operation is instead a compile-time error only when
its pointer operand is an expression-local compile-time constant known to be
null or misaligned. This required determination may evaluate literals, casts,
pointer arithmetic, and operators within that operand expression, but it does
not propagate facts through local bindings, assignments, branch conditions,
reachability, or function calls. Broader analysis may produce warnings but does
not make an otherwise accepted operation a compile-time error.

The remaining obligations cannot in general be checked by the implementation.
Violating provenance, liveness, arithmetic extent, subtraction/ordering
compatibility, bounds, initialization, pointee-type, write-permission, or
concurrent-access requirements is undefined behavior. In particular,
accidental retention by the conservative collector and later address reuse
cannot make a dangling raw pointer valid. An implementation may diagnose or
trap additional violations, but a program cannot rely on it doing so.

Converting a raw pointer to a safe reference asserts that all of the raw
pointer obligations will remain satisfied for every use while the resulting
reference is reachable. Once constructed validly, a reference to
language-managed storage becomes a strong path as described in Section 9. A
reference to foreign or manually managed storage does not extend that storage's
lifetime, so unsafe code that constructs such a reference is responsible for
preventing it from outliving the foreign storage. Safe code alone cannot create
undefined behavior through a raw pointer because it cannot dereference one or
convert one to a reference. If a later safe reference use observes a violated
foreign lifetime contract, the undefined behavior is attributable to the
earlier unsafe construction of that reference.

## 4. Types

### 4.1 Primitive, tuple, and string types

Elamite provides `bool`, `char`, `()`, signed and unsigned fixed-width integer
types, `f32`, and `f64`.

| Category | Types |
| --- | --- |
| Signed integers | `i8`, `i16`, `i32`, `i64`, `i128`, `isize` |
| Unsigned integers | `u8`, `u16`, `u32`, `u64`, `u128`, `usize` |
| Floating point | `f32`, `f64` |
| Other | `bool`, `char`, `()` |

Integer literals support decimal notation and `0b`, `0o`, and `0x` prefixes.
Integer and floating literals may contain `_` separators and may carry a numeric
type suffix. A separator must occur between two digits of the same digit run;
it cannot begin or end the run or appear twice consecutively. An unsuffixed
integer literal materializes as an expected integer or floating type when
representable and otherwise defaults to `i32`. A floating literal contains a
decimal point or exponent, accepts an `f32` or `f64` suffix, uses an expected
floating type when present, and otherwise defaults to `f64`. Unary `-` is an
operator rather than part of a literal token, but literal range checking
includes an immediately applied minus so each signed type's minimum value is
expressible.

Concrete numeric types never convert implicitly, and arithmetic operands must
have compatible concrete types after literal materialization. `value as Type`
performs an explicit numeric conversion. Integer-to-integer conversion traps
when the value is out of range. Float-to-integer conversion truncates toward
zero and traps for NaN, infinity, or an out-of-range result. Integer-to-float
and float-to-float conversion use IEEE rounding. Boolean, character, and enum
values do not participate in numeric casts.

The standard library provides `Type.try_from(value)` for a nontrapping checked
conversion and `Type.wrapping_from(value)` and
`Type.saturating_from(value)` where those behaviors are meaningful. Checked
conversion returns `Result[Type, NumericError]`.

Integer arithmetic traps on overflow, division by zero, division of a signed
minimum by `-1`, and invalid shift counts in every build. Standard operations
such as `checked_add`, `wrapping_add`, and `saturating_add` provide explicit
alternatives; corresponding operations exist for the other arithmetic
operators where meaningful. Checked arithmetic returns `Option[T]`.
Floating-point arithmetic follows IEEE 754. A statically evident invalid
literal, conversion, or arithmetic operation is a compile-time error. `isize`
and `usize` use the selected C target's pointer width.

Tuples use parentheses, for example `(bool, String)`. `()` is both the unit
value and the empty tuple. `(value)` is a grouped expression, while `(value,)`
is a one-element tuple.

A zero-based positional selector accesses a tuple component with ordinary
postfix precedence: `pair.0`, `pair.1`, and so on. The selector is a canonical
unsuffixed decimal integer with no sign, radix prefix, separator, or leading
zero except `.0`, and it must be statically within the receiver tuple's arity.
Numeric selectors do not name struct fields or tuple-like enum payloads and
are never dynamic indices or method names. Existing floating-point literal
tokenization is unchanged.

Positional access composes left-to-right with every other postfix operation;
the receiver in `callback().0`, `value.0.name`, or `values.1[index]` is
evaluated exactly once. In value context, access produces an ordinary shallow
copy of the component. When rooted in an addressable tuple path, it is an
addressable place; when that path is mutable it is also assignable, supporting
replacement, compound assignment, nested mutation, `&pair.0`, and
`&var pair.0` under the ordinary place and promotion rules. A safe-reference
receiver is automatically dereferenced as for a named field. A raw-pointer
tuple receiver may be selected directly only in an `unsafe` context; selection
performs the same mandatory null and alignment checks as explicit
dereferencing, and only `*var Tuple` produces an assignable raw target. A
reference stored as a tuple component receives no special dereference behavior.

`str` is an immutable UTF-8 character sequence.
`String` is the standard-library mutable UTF-8 sequence type. Copying a
`String` copies its backing descriptor shallowly; content mutation is visible
through every descriptor that still reaches the same bytes, while replacing
one descriptor does not replace another. `str` qualifies for `StableHash`;
`String` does not.

A string literal materializes as `str` or `String` when an expected type is
available from a binding annotation, field, argument, or return position. With
no expected type, its type defaults to `str`. Contextual literal materialization
does not create a general implicit conversion: an existing `str` value must use
an explicit operation such as `String.from(text)` to produce a `String`.
Replacing a `str`-typed field through a mutable aggregate path is valid, but the
contents of an existing `str` value cannot be mutated. Formatting is defined in
Section 7.

Ordinary string literals use double quotes, and character literals use single
quotes. They may contain Unicode scalar values directly. The supported escapes
are `\\`, `\"`, `\'`, `\n`, `\r`, `\t`, `\0`, and `\u{HEX}`, where `HEX`
contains one through six hexadecimal digits and denotes a valid Unicode scalar
value. `\"` is primarily useful in strings and `\'` in characters, but both
are accepted in either literal. A character literal must decode to exactly one
Unicode scalar value. A physical newline cannot occur inside either literal.
Other escapes, invalid Unicode scalar values, and unterminated literals are
compile-time errors.

A fixed array type is `[T; N]`, where `N` is a compile-time nonnegative `usize`
value. `[first, second]` constructs an array. The compiler-handled built-in
macro forms `@vec[first, second]`, `@map{key: value, ...}`, and
`@set{value, ...}` construct a `Vec`, `Map`, and `Set`, respectively. The
`@name` namespace is reserved for macro invocation. These three built-ins
share the stable macro namespace described in Section 12.
Their lowercase macro names are distinct from the `Vec[T]`, `Map[K, V]`, and
`Set[T]` type names.

Literal elements and map entries are evaluated left-to-right. Their types must
produce one exact element, key, or value type after contextual literal
materialization. An empty array, vector, map, or set literal requires an
expected collection type. Multiline collection literals permit trailing
commas. A later duplicate map key replaces the earlier value, while duplicate
set elements collapse to one element.

Arrays are ordinary fixed-size aggregates and shallow-copy their inline element
slots. An element that contains a descriptor or handle preserves its backing
identity. Arrays qualify for `StableHash` when their element type does.
`Vec.new()`, `Map.new()`, and `Set.new()` are the ordinary associated functions
for empty collections; populated construction uses the corresponding literal
form.

The standard-library growable sequence type is `Vec[T]`. `Vector` is not an
alternative name for this type.

Copying a `Vec[T]` copies its backing pointer, length, and capacity. Element
writes through any copy are visible through every descriptor whose range
contains that element. Length and capacity belong to each descriptor: an
append changes only the receiver descriptor's length, reuses shared backing
when capacity permits, and otherwise gives that descriptor newly allocated
backing. Whether two vector descriptors continue sharing after growth may
therefore depend on allocation history.

Copying a `Map[K, V]` or `Set[T]` preserves the identity of the complete table,
including its current length. Insert, replacement, removal, and `clear` through
one copy are visible through every copy. Copying an aggregate containing any of
these collections follows the same shallow rule for its collection fields.

`Map[K, V]` keys and `Set[T]` elements must have the compiler-controlled
`StableHash` capability. `StableHash` guarantees that equality and hashing do
not change while a value is stored in a hashed collection. It is inferred
structurally rather than implemented with an ordinary `impl`: integral
primitives, `bool`, `char`, `str`, and `()` qualify, and tuples, structs, and
enums qualify when every field participating in equality and hashing qualifies.
Mutable aggregate types such as `String`, `Vec`, `Map`, and `Set` do not
qualify. Neither `&T` nor `&var T` qualifies for content-based hashing because
another alias may mutate the target. Floating-point types do not qualify.

`Map` values have no `StableHash` requirement. No collection API exposes a safe
reference to any collection interior. The standard-library
`Identity[&T]` and `Identity[&var T]` wrappers compare and hash target identity
rather than target contents and are compiler-known exceptions that qualify for
`StableHash`. They are formed explicitly with
`Identity[ReferenceType].from(reference)`; the argument's safe-reference type
must exactly match `ReferenceType`.

Array and `Vec` indices have type `usize`. Indexing either in value context
produces an ordinary shallow copy of the selected element. An out-of-bounds
index traps; an index that is statically known to be out of bounds for an array
is a compile-time error. Through a mutable collection path, indexing may select
an assignable element for replacement, compound assignment, or direct
nested-field mutation, but never for safe-reference formation. Arrays provide
`len() -> usize` and `get(index) -> Option[T]`. Their length never changes, and
they have no structural mutation operations.

`Vec` provides `len() -> usize`, `is_empty() -> bool`,
`get(index) -> Option[T]`, `append(value) -> ()`,
`insert(index, value) -> ()`, `remove(index) -> T`, and `clear() -> ()`.
Insertion accepts an index from zero through the current length, inclusive;
removal requires an index below the current length. An invalid index traps, and
`remove` returns a shallow copy of the removed element.

Indexing a `Map[K, V]` with a `K` in value context shallow-copies the stored
value and traps when the key is absent. Through a mutable map path, an
indexed value may be replaced or directly mutated as an assignable place, but
it is not addressable for reference formation. An indexed mutable place requires
an existing key and traps if the key is absent; insertion uses `insert`. Map key
arguments are passed by the language's ordinary copy semantics. `Map` provides `len() -> usize`,
`is_empty() -> bool`, `contains_key(key) -> bool`,
`get(key) -> Option[V]`, `insert(key, value) -> Option[V]`,
`remove(key) -> Option[V]`, and `clear() -> ()`. `insert` returns a shallow copy
of the replaced value, if any; `remove` similarly returns the removed value.

`Set` has no indexing operation. It provides `len() -> usize`,
`is_empty() -> bool`, `contains(value) -> bool`, `insert(value) -> bool`,
`remove(value) -> bool`, and `clear() -> ()`. Its value arguments use ordinary
copy semantics. `insert` returns whether the value was newly added, and
`remove` returns whether a value was present.

### 4.2 Structs

`struct` declares an aggregate value type. Fields must appear before methods in
the struct body. A struct's inherent methods are declared in that same body;
there is no inherent `impl` block. Fields and inherent methods share one struct
member namespace, so a field and an inherent method cannot have the same name.

~~~elx
struct Session:
    active: bool
    name: String

    pub fn new(name: String) -> Self:
        return Self { active: true, name: name }

    fn stop(self: &var Self) -> ():
        self.active = false
~~~

Within a struct body, `Self` denotes the enclosing struct type. A plain
`self: Self` parameter receives a copied receiver. `self: &Self` and
`self: &var Self` receive shared and mutable references respectively.
`self: *Self` and `self: *var Self` receive const and mutable raw pointers
respectively. These five forms are the only permitted types for a parameter
named `self`; other pointer types and other parameterized types must use an
ordinary parameter name. The same receiver forms are available to trait
methods.

A bound call such as `value.method()` adapts only its receiver. If the method
expects `self: Self`, the receiver expression is evaluated exactly once and
copied into `self` using ordinary value semantics. The receiver need not be
addressable, and its source remains valid after the call. A receiver of type
`&Self` or `&var Self` is automatically dereferenced and its target is copied,
consistent with ordinary reference method access.

If the method expects `self: &Self`, an addressable value receiver is
automatically borrowed as `&value`. If it expects `self: &var Self`, an
addressable mutable value is automatically borrowed as `&var value`. A receiver
that is already a suitable reference is used directly. Receiver adaptation
never upgrades `&T` to `&var T`, and it does not apply to any non-receiver
argument.

Postfix field selection and calls compose from left to right. In
`value.name(arguments)`, member lookup first checks for a field named `name`.
When that field exists, the expression selects its value and applies ordinary
call syntax to it; the field's type must therefore be callable, and no receiver
adaptation occurs. If no such field exists, the expression performs bound-method
lookup. A field takes precedence over a same-named trait method in lexical
scope; explicit trait qualification selects the trait method instead.

For a method whose receiver is `self: *Self` or `self: *var Self`, a bound call
requires a receiver that already has that exact raw-pointer type. The pointer is
passed unchanged: bound-call resolution does not borrow, cast, downgrade,
dereference, or check it for null or alignment. Calling a method through a raw
pointer therefore does not by itself access the pointee. Any dereference or
raw-to-reference conversion in the method body still requires an explicit
`unsafe:` context, and calling a method declared `unsafe` still requires an
`unsafe:` context at the call site. A raw-pointer receiver never adapts to a
method expecting `self: Self`.

Struct literals use `Type { field: expression, ... }`. Fields may appear in any
order but every field must appear exactly once. `Type { field }` abbreviates
`Type { field: field }`. Multiline comma-separated forms permit a trailing
comma. Elamite initially has no record-update or spread expression.

Every cycle in the value-containment graph of structs and enums must cross an
explicit indirection type: `&T`,
`&var T`, `*T`, or `*var T`. Generic wrappers such as `Option[T]` and `Vec[T]`
and transparent type aliases do not break a containment cycle. This rule makes
recursive identity, aliasing, and mutability visible in source types. Hidden
managed backing used to implement a descriptor-bearing standard type does not
count as explicit indirection.

~~~elx
struct Chain[T]:
    value: T
    next: Option[&Chain[T]]

struct MutableChain[T]:
    value: T
    next: Option[&var MutableChain[T]]

let leaf = Chain { value: 1, next: Option.None }
let root = Chain { value: 2, next: Option.Some(&leaf) }

// Invalid: the recursive path does not cross explicit indirection.
// struct Node:
//     next: Option[Node]
~~~

### 4.3 `Default` derivation and initializers

The general derivation form is an attached `@derive(...)` attribute immediately
before a struct or enum. Its argument is a nonempty, comma-separated list of
derive names, as in `@derive(Default, PartialEq)`. Duplicate entries are
invalid, including duplicates introduced through aliases. Section 12 defines
user-written derive declarations, lookup, execution order, and validation.

The existing compact form places compiler-supported derives in parentheses
immediately after the declaration name and any generic parameter list, as in
`struct Wrapper[T](Default, PartialEq):`. It remains an ungated compatibility
form for the compiler-supported derive inventory. It cannot name a user-written
derive. Mixing compact and attached derivation on one declaration is invalid.
A derived implementation has no visibility modifier separate from its type
declaration.

`Default` is a built-in trait with the associated function
`fn default() -> Self`. `@derive(Default)` on a struct, or the compact
`struct Name(Default):` form, derives an implementation that supplies
`Self.default()` by calling `default()` for each field. Derivation is valid only
when every field type implements `Default`. For a generic struct, the derived
implementation exists conditionally when the field types it uses satisfy that
requirement; derivation does not add bounds to the type declaration itself.
`Default` derivation is struct-only. An enum may implement `Default` manually,
but no variant is selected implicitly.

A `new` method may call `default()` to construct an instance. `new` is an
ordinary associated-function name, not a keyword or allocation expression. A
function named `new` has exactly the parameters, return type, and behavior
declared by its definition.

~~~elx
struct Point(Default, PartialEq):
    x: f64
    y: f64

    pub fn new() -> Self:
        return Self.default()
~~~

The standard defaults are zero for numeric types, `false` for `bool`, Unicode
U+0000 for `char`, `()` for unit, empty values for `str`, `String`, `Vec`,
`Map`, and `Set`, and `null` for both raw-pointer types. Tuples default
fieldwise. `Option[T]` defaults to `Option.None` without requiring `T` to
implement `Default`.

Safe references and function references do not implement `Default`. A struct with
a direct safe-reference field therefore cannot derive it, while a field such as
`Option[&T]` can default to `Option.None`. Ordinary enums do not derive
`Default` because no variant is implicitly preferred; an enum may implement the
trait manually.

### 4.4 Enums, optionals, and aliases

Enums are tagged unions with unit-like, tuple-like, or record-like variants.
`Option[T]` represents a possibly absent value. Elamite has no trailing
optional-type syntax. Struct and enum containment is checked together, so every
recursive cycle must cross an explicit reference or raw-pointer type.

A record-like variant declares fields as `Variant { field: Type }` and is
constructed as `Enum.Variant { field: value }`. Its construction follows the
same field ordering, uniqueness, shorthand, and trailing-comma rules as a struct
literal.

~~~elx
enum Result[T, E]:
    Ok(T)
    Err(E)

enum Option[T]:
    Some(T)
    None

enum State:
    Count(i32)
    Positioned { x: i32, y: i32 }
    Disabled
~~~

A module-level `type` alias is transparent. Generic type parameters and
arguments use square brackets. An alias declares only the type parameters that
remain variable in its replacement type; it may supply concrete arguments for
any other generic parameters of that type.

~~~elx
type NameMap[V] = Map[str, V]
~~~

### 4.5 Equality, ordering, and hashing

`PartialEq`, `Eq`, `PartialOrd`, `Ord`, and `Hash` are compiler-known traits and
may be implemented manually or derived for structs and enums. `==` and `!=`
use `PartialEq`; `<`, `<=`, `>`, and `>=` use `PartialOrd`. `Eq` marks
`PartialEq` as an equivalence relation, while `Ord` supplies a total order and
requires `Eq` and `PartialOrd`. Manual implementations are responsible for
obeying the traits' laws.

Derived comparison is structural. Tuples and struct fields compare in
declaration order. Enum variants order by declaration order and equal variants
then compare their payloads. `Vec` compares lexicographically. `Map` equality
compares key-value mappings and `Set` equality compares elements, independent
of backing-storage or iteration order; maps and sets have no built-in relational
ordering. `str` and `String` compare their exact Unicode code-point sequences
without normalization.

Floating-point values implement `PartialEq` and `PartialOrd` with IEEE behavior,
including unordered comparisons involving NaN. They do not implement `Eq`,
`Ord`, or `StableHash`. Integral primitives, `bool`, `char`, unit, and `str`
provide total equality, ordering, and hashing. Other aggregate implementations
are conditional on the corresponding capabilities of their components.

Safe references compare target storage identity rather than target contents.
Trait-object references likewise compare their concrete target identity. Raw
pointers compare address identity with safe `==` and `!=`, including comparison
with `null`. Raw data pointers additionally have the unsafe relational operators
specified in Section 3.3; those operators are primitive provenance-sensitive
operations and do not implement `PartialOrd` or `Ord`. Safe references and
trait-object references have no relational ordering. Function references
compare their target-function identity as defined in Section 5.
Content comparison through references is explicit, as in
`*left == *right`. Because recursive aggregate edges cross explicit reference
or pointer types and those edges compare by identity, compiler-derived
structural equality terminates for recursive values.

`StableHash` requires a compiler-proven stable structure together with built-in
or compiler-derived `Eq` and `Hash`. Types using manually implemented equality
or hashing do not qualify initially. `Identity[&T]` and `Identity[&var T]`
provide `Eq`, `Hash`, and `StableHash` using the referenced target's stable
managed address, allowing explicit identity-keyed maps and sets.

## 5. Functions and function references

Named function parameters require a name and type. The return type follows the
parameter list with `->`. It may be omitted for a unit-returning function. A
non-unit function must explicitly return a value with `return expression` on
every reachable path. Elamite has no implicit tail-expression return: the value
of an expression used as a statement is discarded even when it is the final
statement in a body. Falling off the end, or using `return` without an
expression, is valid only for a unit-returning function.

The restricted never-return type is written `!`. It is valid only by itself
after the return arrow of a named function declaration or a safe, unsafe, or
raw function type, such as `fn stop() -> !`, `&fn() -> !`, or
`*unsafe fn() -> !`. It is not a general value-bearing type: a field,
parameter, local annotation, alias target, generic argument, aggregate
component, or foreign-function result cannot be `!`. A prefixed form such as
`!i32` is invalid and does not mean “may panic while returning `i32`.”

A function declared `-> !` has no normally returning path. Its body cannot
fall through and cannot contain an ordinary `return`, with or without a value.
Calling a function whose exact return type is `!` terminates that expression
path and produces no value. Such a call may satisfy an expected type only
because control cannot continue to observe a result; at a control-flow join,
normally completing paths determine the joined value type. This bottom
behavior does not introduce function-type subtyping:
`&fn() -> !` and `&fn() -> T` are distinct exact types. Generic substitution,
trait conformance, static dispatch, and trait-object dispatch preserve the
declared return type exactly.

Ordinary runtime traps do not require or imply `-> !`. A function returning
`T` continues to mean that every normal return produces `T`, even when some
executions can terminate through a bounds check, arithmetic trap, explicit
panic, or another unrecoverable runtime condition.

Elamite does not support function overloading. A declaration namespace may
contain only one function of a given name, regardless of parameter types,
return type, or generic parameters. Generic functions and distinct names are
the alternatives for type-specific behavior. This rule does not decide
collisions between inherent and trait methods, which are governed by method
resolution.

Function parameters cannot have default values. Every call to a non-variadic
function must provide exactly its declared number of arguments, so every
non-variadic `&fn(Args) -> Return` value has one fixed arity.

A final parameter may use the variadic form `name: ...T`. It accepts zero or
more trailing arguments, each of type `T`, and binds `name` inside the function
to the slice type `[T]`. Variadics are homogeneous and may appear only once,
as the final parameter. A variadic function value preserves the marker, for
example `&fn(i32, ...String) -> ()`. Elamite lowers this form as a slice
argument rather than as C's untyped variadic calling convention. The packed
arguments use managed backing storage, so the slice remains valid if it is
returned, stored, or captured after the call. A slice is immutable: indexing
and iteration produce shallow copies rather than mutable interior places.
It provides `len() -> usize`, checked indexing, and `for` iteration in index
order.

~~~elx
fn apply_offset(callback: &fn(i32) -> i32, value: i32) -> i32:
    return callback(value)

fn session_status(session: &Session) -> str:
    if session.active:
        return "active"
    else:
        return "inactive"

pub fn variadic(x: i32, y: ...String) -> ():
    for value in y:
        println(f"{x}: {value}")

variadic(7)
variadic(7, "one", "two")
~~~

A named-function value is a *function reference*. A safe function reference is written
`&fn(Parameters) -> Return`; an unsafe function reference is written
`&unsafe fn(Parameters) -> Return`. The bare forms `fn(Parameters) -> Return`
and `unsafe fn(Parameters) -> Return` are function types that, like a trait,
are inhabited only behind a reference. Every value or storage location that
holds a named Elamite function therefore has one of the two reference types.
There is no `&var fn` form, because a function's code is never mutated. A
function reference is produced only by referencing a named function or an
unbound method, and its safety qualifier matches that declaration. Bound-method
values remain unsupported.

Referencing a named function, as in `let bump = increment`, produces an `&fn`
value whose target is that function. A function reference is called with ordinary
call syntax, `bump(args)`, which dereferences it automatically, exactly as
reference field and method access does (Section 3.2).

~~~elx
fn increment(value: i32) -> i32:
    return value + 1

let bump: &fn(i32) -> i32 = increment
println(f"{bump(41)}") // 42
~~~

The corresponding general raw function-pointer types are
`*fn(Parameters) -> Return` and `*unsafe fn(Parameters) -> Return`. They are
not specific to C: they are the raw, nullable counterpart of Elamite function
references and may be stored or passed anywhere an ordinary raw pointer may be
used. There is no `*var fn` form because code is not writable. An exact `&fn`
or `&unsafe fn` may be explicitly converted to its matching `*fn` or
`*unsafe fn`. Function-pointer and data-pointer domains are distinct; casts
between them are invalid.

Both reference and raw function values use ordinary call syntax and are
automatically dereferenced for the call. Calling any raw function pointer
requires `unsafe:`, even when its target signature is safe, because the caller
must establish that the pointer is non-null, valid, and names a function with
the exact signature. An executed null raw-function call traps before
invocation. The `unsafe` marker in `*unsafe fn` additionally records
preconditions imposed by the target function itself.

~~~elx
let raw: *fn(i32) -> i32 = bump as *fn(i32) -> i32
unsafe:
    println(raw(41))
~~~

Referencing a function or unbound method declared `unsafe` instead produces an
`&unsafe fn` value. Taking, storing, copying, passing, returning, or comparing
that reference is safe because none of those operations invokes its target.
Calling it requires an `unsafe:` context, exactly like a direct call to the
declaration.

~~~elx
let recover: &unsafe fn(*Session) -> &Session = Session.get_self_ptr_unsafe

unsafe:
    let session = recover(pointer)
~~~

### 5.1 Explicit-capture closures

A closure is a safe anonymous callable with its own function boundary. A
captureless closure is written `fn(parameters):` or
`fn(parameters) -> Return:`. A capturing closure places a nonempty capture list
before the parameter list:

~~~elx
let offset: i32 = 4
let add = fn[offset](value: i32):
    return value + offset

var total: i32 = 0
let accumulate = fn[&var total as state](value: i32) -> i32:
    *state += value
    return *state
~~~

Closure parameters always have explicit types and cannot be variadic. A closure
literal introduces no generic parameters, cannot be declared `unsafe`, and
cannot capture implicitly. It may occur within a generic declaration; ordinary
substitution then makes the anonymous closure type concrete.

Every enclosing local used by a closure body must occur exactly once in its
capture list. Module declarations, imports, types, and named functions require
no capture. A capture may be renamed with `source as alias`; the alias is the
only binding visible in the closure. Capture aliases and parameters share one
local namespace and cannot collide. The binding receiving a closure is not in
scope while its initializer is resolved, so a closure cannot capture itself or
perform anonymous recursion.

Captures are evaluated exactly once from left to right when execution reaches
the closure expression:

- `value` stores an ordinary shallow copy;
- `&value` forms a shared reference to addressable storage;
- `&var value` forms a mutable reference and requires mutable storage;
- `*pointer` copies a raw pointer, downgrading `*var T` to `*T` when needed;
- `*var pointer` requires and preserves `*var T`.

A raw-pointer local cannot use the plain capture form. Raw-pointer captures do
not dereference the pointer or keep its pointee alive. Their later dereference,
automatic field access, or conversion to a safe reference follows Section 3.3
and requires an explicit `unsafe:` block. Merely copying, storing, passing, or
testing equality on a captured raw pointer remains safe; arithmetic, indexing,
and relational ordering retain their unsafe requirement.

A capture alias cannot be rebound. Mutation through a captured `&var T` or
`*var T` changes the referenced storage. A plain capture owns its copied inline
environment slot, but descriptors and handles in that slot retain their shallow
backing identity. Constructing the closure creates that environment once;
copying the closure value copies its callable descriptor and preserves the
environment identity rather than allocating or copying the environment again.
Closure environments and address-taken captured storage are managed, so a safe
reference may outlive the source stack frame. A raw pointer alone never roots
its pointee.

The return annotation is optional. Explicit `return` expressions and an
expected callable result constrain one exact inferred type; every returned
value must agree. Reachable fallthrough and bare `return` contribute `()`.
There is no implicit tail-expression return. An annotated non-unit result
requires a value on every normally completing path, and `-> !` follows the
ordinary never-return rules.

Each closure expression has a distinct anonymous nominal type and implements
the standard user-implementable `Callable[Arguments, Return]` trait, where
`Arguments` is the exact argument tuple. Ordinary call syntax invokes a
closure. Generic code may call a type parameter through a matching `Callable`
bound, and a callable may be erased behind
`&Callable[Arguments, Return]`. Named safe function references participate in
the same callable contract for direct calls and static `Callable` bounds, but
do not convert directly to `&Callable`. Trait-object erasure requires a safe
reference to nominal storage implementing the trait, such as a referenced
closure value. This keeps function and data pointer domains separate and does
not introduce an implicit allocation or a new storage identity merely to erase
a function address. Closures, including captureless closures, never convert to
`&fn`, `*fn`, or a C callback.

A closure does not inherit an enclosing `unsafe:`, loop, `defer`, or function
return context. Its body begins safe and uses its own `return`, postfix `?`,
never-return, and cleanup rules. It cannot `break` or `continue` an enclosing
loop. Private evolving captures, implicit/default capture, initialized capture
expressions, generic closure literals, unsafe closures, variadic closures,
anonymous recursion, callable equality or hashing, `CallableMut`,
`CallableOnce`, and closure-to-function-pointer conversion are not supported.

A function reference is an ordinary storable value. It may appear in a binding,
field, enum payload, collection element, parameter, or return value. Named
functions, instantiated generic functions, and unbound methods produce function
references that are compatible only when parameter types, return type, arity, and
any variadic marker and safety qualifier match exactly. A safe function
reference does not convert implicitly to an unsafe function reference, and an
unsafe function reference never converts to a safe one. Function types have no
variance or implicit signature adaptation, and collections of them are
homogeneous by complete function type.

A named function has a stable address for the whole program, so its safe or
unsafe function reference is always valid and never requires escape promotion.
A function reference carries no captured environment, so returning or storing
one carries no enclosing-scope state.

A generic function becomes a function reference only after all of its type
arguments are determined explicitly or by an expected function type. Elamite
initially has no erased any-callable type, dynamically erased call-operator,
runtime signature inspection, or heterogeneous function-value collection.
Ordinary trait-object method dispatch is defined separately and does not make a
trait object directly callable with `object(args)`.

Selecting a method from a type produces its unbound function reference. Selecting
a method from an instance does not produce a function reference; an instance
method may be called directly, but a bare expression such as `session.stop` is
invalid. An unbound method retains its declared receiver parameter, so its caller
must form any required reference explicitly. Its function reference also retains
the method's safety qualifier. A trait-qualified method selection is also
unbound and follows the same rule.

~~~elx
let stop: &fn(&var Session) -> () = Session.stop // unbound method
// let handler = session.stop                    // invalid
stop(&var session)
~~~

Because a function reference carries no state, a callback that must carry data
uses a trait object instead. The data lives in a struct, and a `&Trait`
reference dispatches to its method (Section 6).

~~~elx
trait Transform:
    fn apply(self: &Self, value: i32) -> i32

struct AddOffset:
    offset: i32

impl Transform for AddOffset:
    fn apply(self: &Self, value: i32) -> i32:
        return value + self.offset

fn apply_all(transform: &Transform, value: i32) -> i32:
    return transform.apply(value)

let adder = AddOffset { offset: 1 }
let adder_ref = &adder
println(f"{apply_all(adder_ref as &Transform, 41)}") // 42
~~~

Named functions may call themselves and other named functions declared in the
same lexical scope, so direct and mutual recursion use named functions.

Function references support `==` and `!=` as the reference-identity comparisons
defined in Section 4.5: two function references are equal exactly when they name
the same function. They do not compare behavior, and, like other references, they
have no relational ordering.

~~~elx
fn is_even(value: i32) -> bool:
    if value == 0:
        return true
    return is_odd(value - 1)

fn is_odd(value: i32) -> bool:
    if value == 0:
        return false
    return is_even(value - 1)

let first: &fn(i32) -> bool = is_even
let second: &fn(i32) -> bool = is_even
let third: &fn(i32) -> bool = is_odd

println(is_even(4))       // true
println(first == second) // true: same function
println(first == third)  // false: different function
~~~

## 6. Generics and traits

Generic declarations use square brackets. A parameter may have inline trait or
compiler-capability bounds, and `+` separates multiple bounds, as in
`fn inspect[T: PartialEq + Toggle](value: &T)`. `StableHash` is permitted in a
bound position even though users cannot implement it. Elamite initially has no
`where` clauses, default type arguments, const generics, or higher-kinded type
parameters.

A call may infer all generic arguments from its ordinary argument types and
expected result type. The solution must be unique. Otherwise the caller writes
every argument explicitly, as in `equivalent[Point](&left, &right)`; partial
explicit argument lists are invalid. Struct and enum literals may likewise
infer all generic arguments from their fields and expected type.

A generic body is type-checked once using only operations provided by its
bounds. Constructed generic types have exact type identity; generic containers
and references introduce no subtype or variance conversions. The C backend
monomorphizes generic functions, types, and trait implementations for each
concrete instantiation. Recursive generic calls using a finite set of
instantiations are valid, but an unbounded expansion such as `T`, `Vec[T]`,
`Vec[Vec[T]]`, and so on is rejected.

A trait declares behavior and is implemented with `impl Trait for Type`. A
method declaration without a body is required; a method with a body supplies a
default implementation. An implementation must provide every required method
with exactly the declared signature, may override defaults, and may not add
methods absent from the trait. Traits initially contain methods only, with no
associated types or constants.

Calls through concrete types and monomorphized generics use static dispatch.
Trait objects provide dynamic dispatch. A trait object is written as a safe
reference to the trait itself, `&Trait` or `&var Trait`, and initially appears
only in that form. The object is a fat reference containing the managed target
reference and a static vtable.

A trait has no value representation, so a trait name denotes a type only as the
target of a safe reference, as a generic or implementation bound, or as the
trait of an `impl Trait for Type`. A bare trait name in any other type
position — a field, parameter, return, local annotation, type alias, or generic
argument — is an error, as is a raw pointer to a trait object.

~~~elx
fn dispatch(toggle: &Toggle) -> String: // valid
    return toggle.status()

// Invalid: a trait has no value representation.
// fn by_value(toggle: Toggle) -> String
// struct Invalid:
//     toggle: Toggle
// fn by_pointer(toggle: *Toggle) -> ()
~~~

A concrete safe reference automatically becomes a trait-object reference when
an exact `&Trait` or `&var Trait` type is contextually expected, the reference's
target type implements the trait, and the mutability matches. Contextual
conversion applies to annotated binding initializers, assignment sources,
return values, call arguments, and aggregate field or element values. The
equivalent explicit conversion, `reference as &Trait` or
`reference as &var Trait`, remains valid.

This is a targeted trait-object conversion, not a subtype or variance relation:
an uncontextualized concrete reference keeps its concrete type, reference
mutability is never upgraded or weakened by the conversion, and other reference
types still require exact equality.

~~~elx
let session = Session.new("demo")
let session_ref = &session
let toggle: &Toggle = session_ref
println(toggle.status()) // dynamically dispatched
~~~

A trait is object-safe when every method available through the object has an
`&Self` or `&var Self` receiver, has no method-level generic parameters, and
does not otherwise mention `Self` in its parameter or return types. A trait that
fails these rules remains usable with static dispatch but cannot form a
trait-object reference. A generic trait can form an object only after all of its
trait type arguments are concrete. Default methods participate in the vtable.

Trait-object calls dispatch through the vtable, and different concrete target
types may coexist in a homogeneous collection such as `Vec[&Trait]`, whose
elements are converted against the collection's expected element type.
Trait objects initially provide no downcasting, runtime concrete-type
inspection, or multi-trait object composition. Safe-reference reachability and
escape promotion apply to their concrete targets.

A `pub trait` exposes all of its methods wherever the trait is accessible.
Trait method declarations and implementation methods cannot carry separate
`pub` modifiers.

Bound method lookup considers inherent methods and methods from traits in the
current lexical scope. An inherent method wins over a same-named trait method.
If multiple in-scope traits otherwise provide a matching method, the call is
ambiguous.

Explicit `Type.Trait.method` qualification directly selects the named member
from the implementation of `Trait` for `Type`. It bypasses field selection,
inherent-method lookup, and bound trait-method lookup unconditionally, so it may
be used to reach a trait method shadowed by a field or inherent method, resolve
multi-trait ambiguity, or state the intended implementation explicitly. The
qualification is valid only when the names are accessible and the selected
trait is implemented for the type. A trait-qualified method is unbound and
retains its declared receiver parameter, so a call forms any required receiver
reference explicitly. Selecting it without calling produces its unbound
function reference. The same qualification selects receiverless trait
functions, with `Type` identifying the implementation.

An implementation may be declared only in the package that defines either the
trait or the outermost nominal target type. A program may contain only one
implementation of a particular instantiated trait for a concrete target type.
Generic implementations use syntax such as
`impl[T: Bound] Trait for Wrapper[T]:`. Two generic implementations are invalid
when any concrete substitution could make both apply. Elamite initially has no
implementation specialization or negative implementations.

Within a trait declaration, `Self` denotes the type that implements the trait.
Within `impl Trait for Type`, `Self` denotes the implementation target `Type`.
`Self` is invalid outside a struct body, trait declaration, or trait
implementation.

~~~elx
fn equivalent[T: PartialEq](left: &T, right: &T) -> bool:
    return *left == *right

trait Toggle:
    fn status(self: &Self) -> String

    fn category(self: &Self) -> str:
        return "toggle"

impl Toggle for Session:
    fn status(self: &Self) -> String:
        if self.active:
            return String.from("trait active")
        else:
            return String.from("trait inactive")

let session_ref = &session
let dynamic_toggle: &Toggle = session_ref as &Toggle
println(dynamic_toggle.status()) // dynamically dispatched
~~~

## 7. Expressions and control flow

`if`, `else`, `match`, `for`, and `while` use indentation-delimited bodies.
Conditions appear after the keyword. `match` evaluates its scrutinee and chooses
the first matching arm. Each arm uses `Pattern:` followed on the next line by
an indented body.

Refutable patterns appear only in `match` arms. They include `_`, immutable
binding names, primitive and `str` literals, tuples, structs, unit/tuple/record
enum variants, and alternatives separated by `|`. The restricted irrefutable
tuple patterns accepted by local `let` and `var` declarations are defined in
Section 3.1 and do not enable any other match-pattern form at a binding site.
Struct and record-variant match patterns use named fields. Field shorthand such
as `Point { x, .. }` binds `x` and ignores the remaining fields; without `..`,
every field must appear. Alternative patterns must bind the same names with the
same types.

A guarded arm uses `Pattern if condition:`. Its bindings are in scope in the
boolean guard. A failed guard proceeds to the next arm, and guarded arms do not
contribute to exhaustiveness. Pattern bindings receive ordinary shallow copies
and behave as `let` bindings. Matching a reference does not
implicitly dereference it; code matches `*reference` when content matching is
intended.

Every match is exhaustive. Patterns over an infinite domain require a catch-all
binding or `_`. Arms are tested in source order and never fall through. A
statically unreachable arm is a compile-time error.

Control-flow constructs, `unsafe` blocks, and indented bodies are statements,
not value-producing expressions. They complete with unit but cannot appear in
a context that requires a value. Assignment and compound assignment are also
statements and cannot be nested inside expressions. Elamite has no increment or
decrement operator; `++` is instead a binary concatenation operator and `--` is
not an operator. Compound assignment evaluates its destination place exactly
once.

Expression evaluation is left-to-right. A call evaluates its callee or receiver
first and then each argument in source order. `&&` and `||` require `bool` and
short-circuit; `!` is boolean negation. Unary `+` accepts numeric values, unary
`-` accepts signed integers and floating-point values, and `~` accepts integers.
Arithmetic and bitwise operators are initially built-in rather than
user-overloadable; comparison operators use the traits defined in Section 4.5
except for the compiler-recognized raw-pointer equality and unsafe relational
operations in Section 3.3.
`%` accepts integers only. A shift count must have an unsigned integer type and
be smaller than the bit width of the left operand. Chained comparisons such as
`a < b < c` are invalid.

Binary `++` concatenates two values of the same concatenable type. It supports
`str`, `String`, and sequence-like standard-library values, including the
compile-time AST list types in Section 12.2. It creates a new logical value and
evaluates its left operand before its right operand. It is distinct from
numeric `+` and is not a general operation on two `std.ast.Expression` values;
an expression tree is composed with `quote:` and interpolation instead.

Operator precedence from highest to lowest is:

1. Field access, calls, indexing, and postfix `?`.
2. Unary `!`, `~`, `+`, `-`, dereference, `&`, and `&var`.
3. `as`.
4. `*`, `/`, and `%`.
5. `+`, `-`, and `++`.
6. `<<` and `>>`.
7. Bitwise `&`.
8. Bitwise `^`.
9. Bitwise `|`.
10. `<`, `<=`, `>`, and `>=`.
11. `==` and `!=`.
12. `&&`.
13. `||`.
14. Assignment and compound assignment.

### 7.1 Collection iteration

The initial `for` statement directly supports slices, arrays, `Vec`, `Map`, and
`Set`; there is no user-defined iteration protocol or source-level iterator
type yet.
The iterable expression is evaluated exactly once and copied into hidden loop
state using ordinary shallow value semantics. For a vector, the hidden
descriptor fixes the loop length while element replacement through another
descriptor remains visible when both still share backing. Length-changing
vector mutation through any alias while that vector is being iterated is
undefined behavior. Inserting, removing, or clearing a map or set during active
iteration through any alias is likewise undefined behavior; replacing an
existing map value may be observed by a later iteration step.

Slices, arrays, and vectors iterate in index order. Maps yield `(K, V)` pairs,
and sets yield their elements; map and set iteration order is unspecified and
may vary between executions. Each yielded element, key, or value is
shallow-copied into the loop's non-rebindable binding. Iteration exposes no safe
references to collection interiors. It visits only direct elements and does not
recursively traverse targets reached through descriptors or references.

### 7.2 Formatted strings and display

`Display` is a compiler-recognized prelude trait with a formatting method that
writes a value to a mutable standard-library `Formatter`. Users may implement
it normally. Primitive values, `str`, `String`, references to displayable
values, and standard collections of displayable values provide implementations.
Its required method is
`fn fmt(self: &Self, formatter: &var Formatter) -> ()`. The initial formatter
surface provides `formatter.write(text: str) -> ()`; implementations compose
other values by writing a formatted string. Formatter growth uses the managed
runtime allocation boundary.

A formatted string literal uses the prefix `f`, as in
`f"point: {point.x}, {point.y}"`, and produces an immutable `str`. Each
`{expression}` is evaluated exactly once in left-to-right order and its value
must implement `Display`. `{{` and `}}` produce literal braces. Unmatched braces
are compile-time errors. Elamite initially has no width, precision, positional,
or debug-format specifiers. Braces in an ordinary string literal have no
formatting behavior.

The prelude `print` and `println` functions each take one generic `Display`
value. They are ordinary homogeneous generic functions rather than special
heterogeneous variadics. A formatted literal combines multiple differently
typed values before it is passed to either function.

~~~elx
fn value_or[T](node: Option[T], fallback: T) -> T:
    match node:
        Option.Some(value):
            return value
        Option.None:
            return fallback

var retries = 0
while retries < 2:
    println(f"retry {retries}")
    retries = retries + 1

for value in @vec[1, 2, 3]:
    println(f"value {value}")
~~~

## 8. Errors and resource cleanup

Recoverable errors use `Result[T, E]`. Applying postfix `?` to a
`Result[T, E]` is valid only inside a function returning `Result[U, E]` with
the exact same error type. The operand is evaluated exactly once. `Ok(value)`
shallow-copies `value` into the value of the postfix expression. `Err(error)`
shallow-copies `error` and immediately returns `Result.Err(error)` from the
enclosing function.

`?` is the explicit exception to the general requirement that returning from a
function uses `return`. It performs no implicit error conversion. A caller must
convert a different error type explicitly, such as with `match`, before
applying `?`. `Option[T]` is handled with `match` rather than `?`.

`std.panic(message: str) -> !`, also available through the prelude name
`panic`, deliberately terminates the process. The message expression is
evaluated exactly once. The runtime reports `E-RUN-PANIC`, the message, and the
panic call site's source location to standard error, flushes standard error,
and exits unsuccessfully. Panic is unrecoverable, is not represented by
`Result`, and does not guarantee that pending deferred calls run.

### 8.1 Package tests and typed runtime traps

A module-level test declaration is written `test name:` followed by a nonempty
ordinary statement body. A test is private and non-callable, has no parameters,
generic parameters, return annotation, attributes, `pub`, or `unsafe`
modifier, and shares the ordinary module-item namespace. Its stable qualified
name consists of its package-relative module path and declared name. Tests may
use private declarations from their own package under the ordinary visibility
rules. Tests in dependencies are parsed and collected but are never discovered
or executed for the selected package.

A test has an implicit unit result. Reaching the end or executing bare `return`
passes. Returning a value and applying postfix `?` are compile-time errors.
Except for the `expect` construct below, its body follows every ordinary
lexical, typing, trait, safety, FFI, control-flow, copying, and cleanup rule.
Normal `check`, `build`, and `run` parse and collect tests so namespace
conflicts are diagnosed, but do not resolve, type-check, lower, or emit their
bodies. Adding tests therefore does not alter a production artifact.

The standard module `std.testing` declares:

~~~elx
pub trait RuntimeTrap:
    fn code(self: &Self) -> str
    fn message(self: &Self) -> String

pub enum BuiltinTrap:
    Panic
    IntegerOverflow
    DivisionByZero
    InvalidShift
    IndexOutOfBounds
    MissingMapKey
    InvalidNumericConversion
    NullPointer
    MisalignedPointer
    ClosedHandle

pub fn assert(condition: bool) -> ()
pub fn fail[T: Display](message: T) -> !
~~~

`std.trap[T: std.testing.RuntimeTrap](reason: T) -> !` raises an
unrecoverable typed trap. The argument, then `code()` and `message()`, are each
evaluated exactly once. A trap identity is the concrete nominal type of
`reason` together with the exact bytes returned by `code()`; equal code text
from different nominal types does not create equal identities. Trap codes and
messages must be deterministic for deterministic program inputs. Raising a
trap flushes standard error and terminates unsuccessfully without unwinding or
guaranteeing pending `defer` execution.

`BuiltinTrap` implements `RuntimeTrap`. Its variants represent
`E-RUN-PANIC`, `E-RUN-OVERFLOW`, `E-RUN-DIVZERO`, `E-RUN-SHIFT`,
`E-RUN-INDEX`, `E-RUN-KEY`, `E-RUN-CAST`, `E-RUN-NULL`, `E-RUN-ALIGN`, and
`E-RUN-CLOSED`, respectively. Allocation failure is deliberately absent:
out-of-memory termination is not an observable runtime trap. Every built-in
runtime check raises the corresponding `BuiltinTrap` identity. `std.panic`
raises `BuiltinTrap.Panic` while retaining its supplied message.

`std.testing.assert(condition)` evaluates `condition` exactly once and returns
unit when true. False terminates with a structured assertion failure carrying
the call-site location. `std.testing.fail(message)` evaluates and formats its
single `Display` argument exactly once, then terminates with the same assertion
failure category. Assertion failure is not a `RuntimeTrap`, cannot satisfy
`expect`, and does not guarantee pending cleanup. Both functions remain
ordinary callable facilities outside test declarations.

An expected-trap statement is written `expect(selector):` followed by a
nonempty ordinary statement body. It is permitted only directly or indirectly
inside a test body. `selector` is evaluated exactly once in the parent test and
its concrete type must implement `RuntimeTrap`. The body executes in a fresh
child process whose initial state is a process copy taken after selector
evaluation. Child mutations, output-buffer state, and cleanup registrations do
not become parent state.

The expectation passes only when the child raises the exact nominal trap
identity and code selected by `selector`. Normal completion, a different
typed trap, assertion failure, OOM, a signal, a foreign crash, or any other
abnormal exit fails the expectation without terminating the parent test
runner. Static errors remain static errors: an `expect` body does not legalize
invalid indexing, conversions, unsafe operations, or other rejected source.
An expectation may not contain another `expect`, and `return`, `break`,
`continue`, postfix `?`, or control escaping its body is invalid.

`elamc test [PACKAGE]` discovers tests only in the selected package. It checks
the selected test bodies, orders them by qualified name, and executes every
test behind a fresh process boundary. Tests cannot communicate through
in-process mutable state; output from each process is captured and reported
with that test. Exact-name and substring filters preserve qualified-name order.
No tests is a successful run, while an explicitly supplied filter matching
nothing is a command-selection error.

The test command accepts the same target, optimization, C compiler, native
flag, and output-directory policy as a native build. A passing run, including
zero discovered tests, exits zero. One or more test failures exit one after all
selected tests have run. Compilation, configuration, toolchain, or empty-filter
selection errors exit two. The legacy fixture matrix is a separate developer
command, `elamc conformance SUITE`, and is not affected by package test
discovery.

~~~elx
fn increment_result[E](result: Result[i32, E]) -> Result[i32, E]:
    let value = result?
    return Result.Ok(value + 1)
~~~

Elamite has no implicit destruction protocol. Garbage collection manages memory
only. Deterministic external-resource cleanup uses the lexical `defer`
statement.

There is no compiler-known cleanup trait or privileged cleanup method name.
Resource types expose ordinary safe unit-returning methods such as `close`,
`release`, or `unlock`, and `defer` may register any such call. Each resource
API defines whether its cleanup is idempotent and whether copied handles share
state. Shared identity must be represented explicitly by the handle's value,
such as through a safe reference, rather than being introduced by a trait
implementation.

A resource that needs fallible flushing, committing, or finalization provides a
separate explicit operation returning `Result`. Callers perform that operation
before scope exit when they need to handle its error; an infallible release
method may still be deferred.

`defer call` registers one safe function or method call to execute when control
leaves the current lexical block. The call must return unit. A `defer`
statement is permitted only in an executable body, and registration occurs only
when control reaches it. It is not a function value or closure, creates no
captured environment, and cannot escape its block. The `defer:` block form
below defers several statements under the same rules.

The deferred call is evaluated when the block exits, using the values its
callee, receiver, and argument expressions have at that time. The bindings
referenced by those expressions remain alive until the call finishes. Reassigning
a `var` after registration therefore affects the later call; a `let` is the
usual choice when deferring cleanup of one particular resource.

Deferred calls execute when their block falls through or is exited by
`return`, `?` propagation, `break`, or `continue`. Calls registered in one block
execute in reverse registration order, and an inner block's calls execute before
those of an enclosing block. A return expression or propagated error is
evaluated and its result copied before deferred calls begin. Consequently,
unconditionally deferring `close()` on a resource that is returned from the
same block closes the returned handle as well; conditional error-only deferral
is not part of the initial language.

A `defer` statement has two forms. `defer call` registers a single call, as
above. `defer:` introduces an indented block whose statements are all deferred
together, for cleanup that needs more than one call.

~~~elx
let file = File.open("report.txt", "w")?
defer file.close()

let left = Buffer.new()
let right = Buffer.new()
defer:
    left.release()
    right.release()

file.write("Elamite report")?
~~~

A `defer:` block is one registration, not one per statement. It registers when
control reaches it and executes as a unit at scope exit, in reverse
registration order with respect to other `defer` statements in the same block.
Its body is an ordinary lexical scope: a binding declared inside it is local to
the deferred block.

Because a deferred block runs while its enclosing scope is already exiting, it
cannot itself change where control goes. `return`, `break`, `continue`, and
postfix `?` are invalid inside a `defer:` block, as is a nested `defer`
statement. A `defer` statement is also invalid inside an `unsafe` block, and an
`unsafe` block is invalid inside a `defer:` block: deferred cleanup is safe
code, and unsafe scopes stay straight-line.

The initial language has no `errdefer` and no conditional error-only deferral. A
direct unsafe or foreign call cannot be deferred; native cleanup is wrapped in
a safe unit-returning method. An explicit panic, an unrecoverable trap
including one during deferred execution, and out-of-memory termination do not
guarantee that that block's remaining deferred statements will run.

Leaving a scope does not implicitly call a resource-cleanup method; only an
explicitly registered deferred call runs. Garbage collection likewise never
calls resource-cleanup methods. A resource that is neither explicitly released
nor registered with `defer` may therefore leak its external resource. An
implementation may warn about leaks it can prove locally, but such diagnostics
are not required to be complete.

## 9. Garbage collection

Elamite's initial runtime uses the non-moving Boehm garbage collector for
managed memory. The compiler accesses it through a collector-neutral runtime
interface so a future implementation may select another strategy, including
reference counting with cycle detection, provided every semantic guarantee in
this section remains unchanged. Stack versus managed-heap placement is
unobservable in safe code. Escape promotion preserves safe-reference behavior
and `Identity` identity. Once created, a managed allocation does not move
during its lifetime.

Managed storage remains alive while it is reachable through a strong language
path. Strong roots include every local binding until its lexical scope ends,
function parameters for the complete call, temporaries until their full
expression finishes, module-level values, safe references, and managed handles
stored inside structs, enums, collections, and hidden loop state.
Assigning a new value to a `var` removes the binding's strong path to its
previous value. Any other strong path to the previous value remains effective.

Reachable safe references are roots for language-managed targets. A reference
constructed from a raw pointer roots the target only when that target is
language-managed. It does not extend the lifetime of foreign, manually
managed, or otherwise external storage; maintaining that storage's validity is
part of the unsafe conversion's obligation.

Raw pointers are not language-level roots. Code retaining a raw pointer to
managed storage is responsible for keeping the target alive through another
managed path. An in-scope binding provides such a path because bindings remain
roots until lexical scope exit. Boehm may conservatively retain an otherwise
unreachable allocation because a bit pattern resembles its address, but a
program cannot rely on that accidental retention.

Cycles without a path from a strong root are unreachable and eligible for
collection. Collection may occur at any implementation-selected point, and no
collection is guaranteed before process exit. Unreachable storage may be
retained indefinitely, particularly because the collector is conservative.
Collection timing and memory usage are not deterministic program behavior.

The initial language has no `Weak` type or other weak-reference facility. It
also has no GC finalizers, implicit destruction protocol, or user-defined
collection callbacks. Garbage collection never invokes resource-cleanup
methods. The runtime may perform internal reclamation work only when it invokes
no user code and creates no observable external-resource cleanup behavior.

Managed allocation failure is unrecoverable because construction, collection
growth, formatting, and escape promotion may allocate implicitly.
Before reporting out-of-memory, the runtime must attempt a full collection. If
allocation still fails, it terminates the process with an out-of-memory
diagnostic. OOM is not represented by `Result`, cannot be caught, and does not
run resource cleanup. A safe allocation never produces `null`.

The initial standard library exposes no portable collection-control or
heap-statistics API. Implementations may offer nonportable diagnostic flags for
collection tracing, leak investigation, approximate heap statistics, and
best-effort forced collection. Such tooling cannot establish stronger
reclamation guarantees or change language-visible values.

## 10. Unsafe operations and C interoperability

An unsafe function is declared with `unsafe`. Its declaration means that every
caller must satisfy the function's documented safety preconditions, so calling
it requires an `unsafe:` block. The function body does not implicitly become an
unsafe context: every unsafe-only operation in the body must still appear in an
explicit nested `unsafe:` block. This keeps the implementation's individual
unsafe assumptions locally visible. Referencing an unsafe function or unbound
unsafe method does not call it and is therefore safe; the resulting
`&unsafe fn` reference preserves the call-site requirement defined in Section
5.

Whether a function is declared `unsafe` depends only on its caller contract,
not on whether its body contains an `unsafe:` block or where a `return`
statement appears. A safe function may use unsafe-only operations internally
when it establishes every required obligation without relying on its caller.
Conversely, a function must be declared `unsafe` whenever sound use requires
the caller to establish an obligation that its parameter and return types do
not express, even if the function performs no unsafe-only operation itself or
returns outside an `unsafe:` block.

An `unsafe:` block permits unsafe-only operations, including raw-pointer
dereference, raw-pointer conversion to a reference, and calls to unsafe or
foreign functions. It does not disable type checking or prove an operation
valid. The author of the block asserts that every operation's documented
preconditions and the raw-pointer obligations in Section 3.3 hold.

~~~elx
unsafe pub fn from_pointer(pointer: *Session) -> &Session:
    unsafe:
        return pointer as &Session
~~~

Using an unsafe-only operation outside the unsafe context required by Section
3.3 is a compile-time error. The expression-local constant rule in Section 3.3
is the only mandatory value analysis for raw-pointer access. A compiler may
warn when broader analysis indicates a violation of provenance, liveness,
bounds, initialization, pointee-type, or write-permission obligations, but
inability to prove valid foreign input is not itself an error. These diagnostics
do not apply merely because a safely formed reference to a local binding
escapes; such storage is promoted.

The consequences of violations not established statically, including the
required null and alignment traps and otherwise undefined behavior, are
specified in Section 3.3.

### 10.1 Foreign declarations and ABI types

The initial foreign-function interface supports only C's platform ABI. It uses
compiler-defined item attributes rather than an `extern` block or an ABI
modifier in the function grammar:

~~~elx
@importc("FILE", "stdio.h")
type FileHandle

@importc("div_t", "stdlib.h")
struct CDiv:
    quot: i32
    rem: i32

@importc("fopen", "stdio.h")
fn open_file(path: *u8, mode: *u8) -> *var FileHandle

@importc("fclose", "stdio.h")
fn close_file(file: *var FileHandle) -> i32
~~~

`@importc("c_name", "header.h")` is valid only on a module-level bodyless
function, opaque bodyless type, or struct. Its first string is the exact C
symbol or type spelling used by generated C; a foreign type may name either a
typedef identifier or a `struct tag`. Its second string names the authoritative
C header, which generated C includes. The local Elamite declaration name may
differ from the C name. Attribute arguments are string literals, and a header
name is restricted to portable ASCII path characters. Importing a declaration
has no runtime initialization effect.

The header remains authoritative for C layout and declarations: the backend
does not emit a competing definition. The Elamite declaration is the type
checker's view of that contract. Header search paths, library search paths,
native libraries, and final link options are declared under `[native]` in
`elamite.toml`:

~~~toml
[native]
include_paths = ["native/include"]
library_paths = ["native/lib"]
libraries = ["example"]
link_options = ["-pthread"]
~~~

An opaque foreign type has unknown size and alignment and may be used only
behind a raw pointer. A foreign struct has the field order, alignment, padding,
and by-value calling convention of the corresponding C struct for the selected
target. It is an ordinary copyable Elamite value, but its fields must themselves
be ABI-safe and it cannot be generic, derive traits, contain methods, or contain
an incomplete opaque type directly. The declaration author is responsible for
matching the C header exactly. A mismatched foreign declaration is an unsafe
contract violation and causes undefined behavior when used across the boundary.

The ABI-safe scalar types are `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`,
`u64`, `isize`, `usize`, `f32`, and `f64`. Raw data pointers, foreign structs
made recursively from ABI-safe fields, and raw `*fn` or `*unsafe fn` pointers
whose parameters and result are ABI-safe are also ABI-safe. A bare function
type or safe function reference is not. Unit `()` is permitted only as a
function return and is lowered to C `void`. The Elamite never return type `!`
is not permitted in an imported C function signature; the initial interface
does not infer non-returning behavior from C declarations. The fixed-width
integer types use the corresponding `stdint.h` ABI types; `isize` and `usize`
use `intptr_t` and `uintptr_t`. The compiler-known opaque type
`std.ffi.CVoid` may be used only
behind a raw pointer and corresponds to C `void`; unsafe raw-pointer casts to
and from `*std.ffi.CVoid` or `*var std.ffi.CVoid` preserve provenance under
Section 3.3.

`bool`, `char`, `str`, `String`, safe references, function references, tuples,
arrays, ordinary structs and enums, trait objects, and standard collections are
not ABI-safe. Neither are `i128` and `u128`, whose C ABI is not portable. There is
no implicit string, collection, nullable-value, or aggregate marshalling.
Wrappers must explicitly encode text into a documented byte representation,
add any required terminator, pass a raw pointer and length, and keep or register
the backing storage for as long as foreign code may access it.

Every imported foreign function is unsafe to call, including one whose
signature contains only scalars. Its declaration has no body, cannot use
Elamite's homogeneous variadic parameter syntax, and its value has the
corresponding raw function-pointer type. C variadic functions are not supported
in the initial interface. Each raw-pointer parameter and result must have a
documented foreign contract covering nullability, readable or writable extent,
alignment, pointee initialization and type, whether the pointer is retained,
and what event ends its validity. The compiler does not infer those facts from
a C header.

### 10.2 Ownership, retention, and managed roots

A raw pointer never owns its target. Receiving an owning native handle through
a raw pointer does not schedule cleanup, and passing a raw pointer never
transfers ownership by itself. An owning C API is wrapped in an ordinary
Elamite handle type whose methods enforce the API's state rules. Such a wrapper
may provide an idempotent `close()` method that invokes the native release
operation. If wrapper copies share one resource state, the wrapper represents
that identity explicitly, such as with a safe reference. Borrowed foreign
pointers remain valid only for the duration promised by their foreign contract.

Safe references are not ABI-safe and cannot cross the C boundary directly.
Code explicitly converts one to a raw pointer. When a foreign call does not
retain that pointer, the source binding or reference remains a strong root for
the complete call. If foreign code may retain a pointer to language-managed
storage after the call returns, the storage must first be registered with the
runtime through `std.ffi.ForeignRoot[T]` or
`std.ffi.ForeignRootMut[T]`.

`ForeignRoot.retain(&value)` and `ForeignRootMut.retain(&var value)` promote the
target when necessary and create a runtime root registration. Their `pointer`
methods return `*T` and `*var T`, respectively. Copies of a foreign-root handle
share one registration, represented by explicit shared state. Their `close()`
method is idempotent, and closing any copy unregisters the root. Calling
`pointer()` on a closed handle traps with `E-RUN-CLOSED`. If every handle
becomes unreachable without being closed, the registration and target may
leak; garbage collection never unregisters it.

Closing a registration is valid only after the foreign contract says that no
later access will occur. Once it is closed, some other strong path may happen
to keep the target alive, but foreign code cannot rely on that fact. A later
foreign access through the retained pointer violates the earlier unsafe call's
contract and has the consequences specified in Section 3.3. A raw pointer
returned by foreign code does not root foreign storage, and converting it to a
safe reference does not extend a foreign lifetime.

### 10.3 Callbacks and foreign control flow

A module-level function definition may use `@exportc("c_name")` to emit its
definition under that exact unmangled C symbol. Its signature must be ABI-safe
and its address is stable for the process lifetime. `@exportc` changes only
external naming and ABI validation: the declaration remains an ordinary safe
or unsafe named Elamite function, and an unsafe function body still requires
explicit `unsafe:` blocks for unsafe-only operations.

~~~elx
@exportc("visit_value")
unsafe fn visit_value(context: *var i32) -> i32:
    unsafe:
        let value: &var i32 = context as &var i32
        *value = *value + 1
        return *value
~~~

Any named function with an ABI-safe signature can serve as a C callback by
explicitly converting its exact `&fn` or `&unsafe fn` reference to a matching
raw function pointer. `@exportc` is needed only when C must link to a stable
symbol by name. Foreign code may retain either address for the process lifetime.
Retained managed callback state is passed separately through a raw context
pointer backed by an open foreign-root registration. The callback function is
responsible for recovering a reference only within an `unsafe:` block, and the
registration must remain open until both callback and context pointer have
been released under the foreign API's contract.

C may invoke an Elamite callback synchronously on the same registered runtime
thread that entered C. This includes the initializer thread and any
Elamite-created thread. Direct and nested reentry and later same-thread calls
are allowed. A foreign-created thread is not registered and cannot enter
Elamite; asynchronous or concurrent callbacks originating on such a thread are
undefined behavior. This restriction is a foreign contract and is not
generally detectable by the compiler.

Recoverable Elamite errors do not cross the ABI automatically; a wrapper must
translate them to an ABI-safe result such as a status code and out-parameters.
Likewise, `errno` and other foreign error channels are observed only through
explicit wrapper operations. A trap reached while executing foreign code or an
Elamite callback terminates the process and never unwinds through C frames.
C++ exceptions, `longjmp`, or any other foreign unwinding across an Elamite
frame are forbidden and cause undefined behavior. Foreign code must catch or
contain them and translate them before returning through the C boundary.

### 10.4 Native threads, shared memory, and synchronization

Elamite exposes native parallelism through ordinary declarations in
`std.thread` and `std.sync`. It adds no thread, task, `concurrent`, `async`, or
`await` grammar. `std.thread.spawn` accepts one safe zero-argument callable,
evaluates it exactly once, shallow-copies its environment, starts one native
thread eagerly, and returns
`Result[std.thread.Thread[R], std.thread.SpawnError]`. Operating-system thread
creation failure is recoverable; allocation failure retains the process-fatal
out-of-memory behavior.

There is no `Transfer` capability and no automatic detachment at a thread
boundary. References, raw pointers, slices, trait objects, strings, collections,
closures, and aggregates preserve the same shallow identities they preserve in
ordinary single-threaded copies. A safe reference remains a managed strong path
when reachable from a registered thread. A raw pointer remains non-rooting, and
its unsafe provenance, lifetime, bounds, alignment, initialization, write, and
synchronization obligations are unchanged when it crosses a thread boundary.

`std.thread.Thread[R]` is a copyable identity handle. Every copy names the same
native thread and cached result. The runtime performs the operating-system join
at most once, and each successful `join()` returns a shallow copy of the cached
`R`; mutable backing in repeated join results may therefore be shared. Joining
the current thread traps. Cyclic joins may deadlock and need not be detected.
Threads are joinable and never implicitly detached; losing every source handle
neither stops nor detaches a thread. After the program entry function returns
normally and its deferred cleanup completes, runtime shutdown waits for every
remaining Elamite-created thread. There is no initial cancellation,
interruption, or detach operation.

A thread body is a safe function boundary only in the lexical sense: it begins
outside `unsafe`, owns its `return` and postfix-`?` context, and runs its `defer`
registrations on ordinary exit. This does not imply data-race freedom. Returning
`Result[T, E]` produces `Thread[Result[T, E]]`; it is not a thread-failure
channel. A runtime trap, `std.panic`, or out-of-memory failure on any thread
terminates the complete process and is never converted to a join result or
unwound through another thread or C frame.

`std.sync.channel[T](capacity: usize)` creates a bounded multi-producer,
multi-consumer channel and returns `(Sender[T], Receiver[T])`. Capacity zero is
a rendezvous channel. `std.sync.unbounded_channel[T]()` creates an unbounded
channel. Sending evaluates its argument once, shallow-copies it into the
message, and reports closure recoverably. Blocking receive returns `Option[T]`,
with `None` only after closure and draining. Nonblocking operations distinguish
full, empty, and closed states. Copies of an endpoint share synchronized
identity. Channel synchronization safely publishes the copied descriptor and
all writes sequenced before the send; it does not synchronize later access to
mutable backing shared by sender and receiver. Closure is explicit and
idempotent; garbage collection or loss of the last visible endpoint never
closes a channel.

`std.sync.Mutex[T]` remains a copyable synchronized identity handle with
`new`, `read`, `replace`, and atomic `update` operations, but shallow copying
makes it a synchronization tool rather than an alias-isolation boundary.
`new`, `read`, `replace`, and the value passed to and returned from `update`
all use ordinary shallow copying. An alias retained outside the mutex may reach
the same backing as its stored value, and the programmer must ensure that every
conflicting access uses a consistent synchronization protocol. Operations on
the mutex serialize only callers using that same handle; the compiler does not
associate a backing allocation with a particular mutex. Recursive locking and
general lock cycles may deadlock. Mutex poisoning is unnecessary because an
unrecoverable thread failure terminates the process.

`std.sync.AtomicBool`, `std.sync.AtomicI32`, and `std.sync.AtomicUsize` are
copyable handles to shared atomic cells. They provide load, store, exchange,
compare-exchange, and the applicable integer read-modify-write operations.
Their operations are sequentially consistent. The C99 backend implements them
through runtime/compiler hooks rather than C11 `_Atomic`, including target-width
`usize` behavior on x86.

Two evaluations conflict when they access the same scalar object or overlapping
bytes and at least one writes. Conflicting evaluations on different threads
that are not ordered by a synchronization edge constitute a data race and make
program behavior undefined. Ordinary collection access needs no `unsafe`
syntax merely because backing is shared: synchronization is the programmer's
responsibility. Bounds checks, managed lifetime, and ordinary type checks remain
in force for executions without undefined behavior, but they do not repair a
data race in the generated C99 program.

Thread creation orders prior evaluations before the new thread begins. A mutex
unlock within an operation orders prior evaluations before a later successful
lock of the same mutex. A successful channel send orders message initialization
and earlier evaluations before the matching receive. Thread completion orders
prior evaluations before a successful `join()` returns. Sequentially consistent
atomic operations participate in one total order. These edges may be composed;
no other ordinary shallow copy creates synchronization.

Scheduling, fairness, relative completion, and cross-thread output-call order
are unspecified. Each complete standard-output call is internally synchronized
so concurrent calls cannot corrupt one another. Blocking synchronization may
deadlock; self-join is the only initially required deadlock trap.

Every runtime-created thread registers with the garbage collector before it
executes Elamite code. Its stack, shallow environment, synchronized queues and
cells, and unpublished or published result remain visible roots. It unregisters
only after publishing its result. Completed thread state becomes reclaimable
after all managed roots to its handles and result disappear.

Cooperative tasks, executors, futures, `async`/`await`, detached execution,
cancellation, interruption, timeouts, thread-local storage, relaxed atomics,
parallel iterators, fairness guarantees, general deadlock detection, automatic
race prevention, and foreign-thread attachment are outside this contract.

## 11. Conformance example

~~~elx
struct Counter:
    value: i32

    fn increment(self: &var Self) -> ():
        self.value = self.value + 1

fn main() -> Result[(), String]:
    var counter = Counter { value: 0 }
    let copied = counter
    let counter_ref: &var Counter = &var counter
    counter_ref.increment()

    println(copied.value)
    println(counter.value)
    return Result.Ok(())
~~~

## 12. Compile-time syntax generation

Elamite has three user-defined compile-time declarations: `macro` produces
syntax at an explicit invocation, `attr` transforms an attached definition,
and `derive` produces an implementation for an attached struct or enum. Their
bodies are ordinary Elamite code executed by the compiler's bounded
compile-time interpreter. They operate on the public `std.ast` model rather
than token matcher/transcriber rules or compiler-private syntax structures.

User-defined compile-time declarations and their uses are stable language
features. The compiler-handled `@vec`, `@map`, and `@set` forms and
compiler-defined `@importc` and `@exportc` attributes remain built-in
compatibility forms.

### 12.1 Declarations, namespaces, and visibility

A compile-time declaration is a documented module-level declaration with an
optional `pub` modifier, a typed parameter list, a return type, and an ordinary
statement body:

~~~elx
pub macro make_pair(
    left: std.ast.Expression,
    right: std.ast.Expression,
) -> std.ast.Expression:
    let pair: std.ast.Expression = quote:
        ($left, $right)
    return pair

pub attr named(
    target: std.ast.StructDefinition,
    name: str,
) -> std.ast.StructDefinition:
    return target.with_name(std.ast.identifier(name))

pub derive Comparable(
    target: std.ast.StructDefinition,
) -> std.ast.Implementation:
    std.ast.error(target, "example derive body omitted")
~~~

`macro` and `attr` declarations bind their declared names in separate macro and
attribute namespaces. A `derive Trait` declaration resolves `Trait` through the
ordinary type namespace and binds the generator under that spelling in a
separate derive namespace. The generated implementation must target that exact
trait identity. Duplicate bindings are diagnosed independently in each
namespace.

Imports use `use macro path`, `use attr path`, and `use derive path`, with the
ordinary `as`, `pub use`, visibility, re-export, reachability, and
noninheritance rules from Section 2.3. Unqualified function-like macro lookup
falls back to the macro prelude containing `vec`, `map`, and `set`. A public
compile-time declaration is distributed as versioned compile-time metadata; it
does not become a runtime declaration or C symbol.

Compile-time declarations and their namespace imports must occur in physical
package source. Generated syntax cannot define or import a `macro`, `attr`, or
`derive`, so expansion cannot mutate its own compile-time environment. These
declarations cannot be local, generic, `unsafe`, foreign, or members of
structs, enums, traits, or implementations. Their signatures may use only the
compile-time value types admitted by `std.ast` and the compile-time interpreter;
they cannot accept or return runtime references, raw pointers, or function
values.

A macro may use the ordinary homogeneous variadic syntax on its final
parameter, for example `arguments: ...std.ast.Expression`. It accepts zero or
more trailing syntax arguments and binds them as `[std.ast.Expression]` in the
body. An attribute may likewise use one final variadic parameter after its
implicit target and any fixed explicit parameters. A derive has exactly its one
implicit target parameter and is never variadic. AST sequences interpolate by
splicing in collection position under Section 12.3, so matcher-specific
repetition syntax is unnecessary.

### 12.2 The `std.ast` model

`std.ast` is a versioned, compile-time-only intrinsic interface. Its values do
not exist at runtime and are not aliases for the compiler's internal parsed,
resolved, or typed AST. They are opaque, immutable owned values with stable
accessors, `with_` transformation methods, structured constructors, pattern
variants, and persistent list types. The initial interface includes at least:

- `Item`, `StructDefinition`, `EnumDefinition`, `FunctionDefinition`,
  `Implementation`, and `FieldDefinition`;
- `Expression`, `StatementList`, `ItemList`, `MemberList`, `Pattern`, and
  `TypeSyntax`; and
- `Identifier` plus opaque origin information suitable for diagnostics.

The model represents pre-resolution structural syntax: written names and type
syntax, visibility, generic syntax, fields, variants, parameters, bodies,
documentation, attributes, and provenance. It does not expose inferred types,
resolved trait selection, layouts, target properties, runtime values, mutable
compiler tables, or arbitrary compiler internals. A later read-only semantic
reflection API, if any, is separate and cannot generate syntax in the same
compilation phase.

AST values can be inspected with ordinary pattern matching, for example
`std.ast.Expression.Call(call)`, and transformed without mutation. Builders
such as `std.ast.literal(value)` and `std.ast.identifier(text)` validate their
input. `std.ast.error(node, message)` emits a compile-time diagnostic at the
node's origin and does not return. User code cannot fabricate a physical span
or call-site syntax context.

### 12.3 Quotation, interpolation, and concatenation

`quote:` introduces an indentation-delimited AST expression. Its expected
`std.ast` type determines whether the quoted body is an expression, pattern,
type, statement list, member list, item, or item list. The expected role may
come from a binding annotation, parameter, or return position; an ambiguous or
incompatible quotation is a compile-time error.

Inside a quote, `$name` interpolates one named AST value and
`$(compile_time_expression)` evaluates and interpolates one computed AST value.
In a collection position an AST list is spliced into the surrounding list; a
scalar inserts one node. `$` has no interpolation meaning outside `quote:` and
is otherwise invalid source syntax.

~~~elx
let members: std.ast.MemberList = quote:
    id: u64

    pub fn identifier(self: &Self) -> u64:
        return self.id

let all_members = target.members() ++ members
let result = target.with_members(all_members)
~~~

Literal syntax written in a quote receives the declaration's definition-site
syntax context. Interpolated syntax retains its existing context and origin.
Consequently, literal generated bindings do not accidentally capture caller
bindings, while interpolated caller-selected names continue to name the
caller's declarations. `++` concatenates AST list values as specified in
Section 7; it does not combine arbitrary expression nodes.

### 12.4 Function-like macros

A function-like macro is invoked as `@path(...)`. Its arguments are parsed as
syntax according to the declaration's parameter types and are not evaluated as
runtime expressions. The declaration's return type fixes its expansion role:

| Return type | Permitted invocation role |
| --- | --- |
| `std.ast.Expression` | one expression |
| `std.ast.Pattern` | one pattern |
| `std.ast.TypeSyntax` | one type |
| `std.ast.StatementList` | one complete statement position |
| `std.ast.Item` or `std.ast.ItemList` | one complete module-item position |

A return type and invocation position that disagree are diagnosed before
execution. Empty output is valid only for statement and item lists. Returned
syntax is validated in its complete role and then receives every ordinary
visibility, coherence, safety, checking, lowering, and backend rule.

~~~elx
macro trace_call(
    expression: std.ast.Expression,
) -> std.ast.StatementList:
    match expression:
        std.ast.Expression.Call(call):
            let description = "calling " ++ call.callee().display()
            let message = std.ast.literal(description)

            let statements: std.ast.StatementList = quote:
                println($message)
                $expression
            return statements
        _:
            std.ast.error(
                expression,
                "`trace_call` requires a function or method call",
            )

@trace_call(load_user(42))
~~~

### 12.5 Attributes

An attribute declaration's first parameter is the implicitly supplied attached
definition. Its type selects the allowed target kind; remaining parameters are
bound from explicit arguments in `@attr(path(...))`. With no explicit arguments
the short form is `@attr(path)`. A same-definition-kind return replaces the
attached definition. `std.ast.ItemList` may replace it with zero or more module
items, allowing an attribute to remove the item or add siblings. Every returned
item is structurally validated before expansion continues.

Attached attributes execute from top to bottom. Each attribute receives the
complete output definition of the preceding attribute. An attribute may add or
change fields, variants, functions, visibility, documentation, and ordinary
attributes, but cannot bypass privacy or any later semantic check.

~~~elx
attr identifiable(
    target: std.ast.StructDefinition,
) -> std.ast.StructDefinition:
    let additions: std.ast.MemberList = quote:
        id: u64

        pub fn identifier(self: &Self) -> u64:
            return self.id

    let members = target.members() ++ additions
    return target.with_members(members)

@attr(identifiable)
struct Entity:
    name: String
~~~

### 12.6 Derives

A derive declaration has exactly one target parameter, typed as
`std.ast.StructDefinition` or `std.ast.EnumDefinition`, and returns
`std.ast.Implementation`. `@derive(Name, ...)` retains the original definition
and invokes the selected generators in source order. All ordinary attributes on
that definition finish first, so derives observe fields, variants, methods, and
other structure added by attributes.

The returned implementation must implement the exact trait named by the derive
declaration for the exact attached type. It may not replace the type or emit
unrelated items. The compiler rejects a mismatched target or trait before
ordinary implementation collection; a valid result then undergoes the same
orphan, overlap, conformance, bound, visibility, and safety checks as a
handwritten implementation.

~~~elx
trait FieldCount:
    fn field_count() -> usize

derive FieldCount(
    target: std.ast.StructDefinition,
) -> std.ast.Implementation:
    let target_type = target.type_syntax()
    let count = std.ast.literal(target.fields().length())

    let implementation: std.ast.Implementation = quote:
        impl FieldCount for $target_type:
            fn field_count() -> usize:
                return $count
    return implementation

@derive(FieldCount)
struct User:
    name: String
    active: bool
~~~

### 12.7 Expansion order, hygiene, and provenance

The compiler first collects physical compile-time declarations and imports for
the selected acyclic package graph. Expansion then uses a deterministic fixed
point ordered by package identity, module path, and source/provenance order.
For each definition, attached attributes run top-to-bottom and derives then run
left-to-right. Function-like invocations expand outermost-first and then
left-to-right; generated ordinary items re-enter the same attachment and
invocation scheduler. Ordinary imports produced by expansion participate in
name resolution only after expansion is complete.

Each execution receives a fresh syntax context. Quote literals use
definition-site context and interpolated nodes retain their prior context.
Definition-site paths, including macro and helper names, resolve from the
defining module; interpolated invocations retain their invocation context.
Hygiene changes lookup context, not access rules.

Every generated node retains an origin chain containing the compile-time
declaration, attachment or invocation, and enclosing executions. Diagnostics
use generated primary locations with related physical invocation and
definition spans. Generated origins are never projected onto fabricated byte
offsets in a physical source file.

Re-entering the same compile-time declaration identity with the same role and
structurally equal input on one active execution chain is a cycle diagnostic.
Recursion with changing input is permitted only until it terminates or reaches
a resource limit. Invalid output, cycles, or nontermination are user-facing
compile-time errors rather than internal compiler failures.

### 12.8 Compile-time execution and limits

The compile-time interpreter implements deterministic safe Elamite semantics
for the subset admitted by compile-time signatures. It has no `unsafe`, FFI,
filesystem, environment, process, network, clock, randomness,
target-introspection, runtime-state, or compiler-internal access. Its result is
therefore a function of the resolved package graph, compile-time source and
dependencies, compiler/specification and `std.ast` interface versions, and the
fixed limits below. Host execution never changes the selected x86 or x86-64
target semantics of the generated program.

One selected package-graph compilation permits:

- at most 128 simultaneously active compile-time executions;
- at most 65,536 total macro, attribute, and derive executions;
- at most 1,048,576 generated AST nodes in aggregate;
- at most 1,048,576 interpreter steps for one execution; and
- at most 64 MiB of live compile-time values for one execution.

The scheduler determines which execution consumes each shared budget. An
execution that exceeds a limit emits a diagnostic at its attachment or
invocation and produces no partial syntax; the compiler may insert an explicit
error node to continue independent diagnostics. Panics, invalid results,
version skew, and resource exhaustion are contained in the same way and cannot
corrupt compiler state or become runtime behavior.
