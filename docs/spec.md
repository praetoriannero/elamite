# Elamite Language Specification

> Status: Implemented draft
>
> Version: 0.11.0-draft
>
> This document is the implemented 0.11 language design. Its authoritative
> executable demonstration is
> [`examples/spec_demo.elx`](../examples/spec_demo.elx), mirrored by the
> [owned-model demonstration](../owned_spec_demo.elx). Implementation evidence
> is mapped in [ledger.md](ledger.md), and unresolved questions are listed in
> [issues.md](issues.md).

## 1. Overview

Elamite is a statically typed, memory-safe language that compiles to C. It
combines indentation-delimited control flow and type inference with owned
values, explicit references, deterministic destruction, traits, generic types,
algebraic data types, recoverable errors, and raw pointers behind an `unsafe`
boundary.

Non-`Copy` values move through assignment, arguments, returns, captures, and
patterns. Retaining an independent value requires an explicit `clone`; taking
`&value` creates a shared borrow and taking `&var value` creates an exclusive
borrow. Elamite has no source lifetime parameters: the compiler infers and
propagates provenance structurally through fields, generic arguments, slices,
closures, and public signatures. Ordinary reference formation never allocates.
Shared identity and graph identity use explicit library types rather than
shallow mutable aliases or a tracing garbage collector. Safe code cannot create
a dangling reference, conflicting exclusive access, double destruction, or a
data race; operations whose contracts cannot be checked remain explicit
`unsafe` code.

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

### 3.1 Ownership, moves, and cloning

`let` creates a non-rebindable binding. `var` creates a rebindable place. Both
bindings own their initialized value unless its type is a borrow, raw pointer,
function reference, or another explicitly non-owning type. A field reached
through a `let` aggregate cannot be assigned through that path and cannot form
`&var`; mutation capability carried by an explicit `&var` field remains usable
without rebinding that field.

A local binding may name one identifier or an irrefutable tuple pattern.
Patterns may nest and contain identifiers or `_`; `()` and `(name,)` are the
empty and one-element forms. An optional annotation applies to the complete
pattern. The initializer evaluates once before any new binding enters scope,
and every identifier is unique.

Assignment, by-value argument passing, return, plain closure capture, pattern
binding, and extraction of an owned field are *consuming contexts*. A
non-`Copy` value moves into the destination and its source place becomes
uninitialized. Using, borrowing, dropping, or moving that unavailable place is
a compile-time error until a `var` place is reinitialized. Moving a value does
not run its cleanup and does not change the address of separately allocated
storage it owns.

A type has the compiler-recognized `Copy` capability exactly when duplicating
its immediate representation creates another independently valid value with no
destruction obligation or exclusive access. Integers, floating-point values,
`bool`, `char`, `()`, shared references, raw pointers, function references, and
aggregates whose fields are all `Copy` are `Copy`. `&var T`, `String`,
collections, owning or shared-ownership handles, and every type with custom
destruction are not `Copy`. A consuming context copies a `Copy` value and leaves
its source initialized.

`Clone` is the ordinary safe prelude trait:

~~~elx
trait Clone:
    fn clone(self: &Self) -> Self
~~~

Calling `value.clone()` is the only general operation that creates another
independently owned non-`Copy` value. A clone may allocate or take time
proportional to reachable owned data; the type's API documents that cost.
`Copy` types implement `Clone` with their ordinary constant-size copy.
The compiler never inserts a `clone` call. Last-use analysis may reuse storage
or improve a move diagnostic, but it cannot change whether source code moves,
copies, borrows, or explicitly clones a value.

A struct or tuple may be partially moved through a statically known field.
Afterward, unaffected fields remain usable through their projections, but the
complete aggregate remains unavailable until every moved field of a `var` place
is reinitialized. A type with custom destruction cannot be partially moved.
Moving through a dynamic array index or ordinary collection index is invalid;
an ownership-taking API such as `remove`, `pop`, or `take` performs that
operation without leaving an uninitialized element.

Tuple destructuring applies the same rule independently to each component:
non-`Copy` components move, `Copy` components copy, and `_` consumes and then
drops an owned component unless the implementation can prove the drop has no
observable effect.

~~~elx
let first = String.from("first")
let moved = first
// println(first)             // invalid: `first` was moved
let duplicated = moved.clone()
println(duplicated)

var pair = (String.from("left"), String.from("right"))
let left = pair.0
println(pair.1)               // the other field remains available
pair.0 = String.from("new")
println(pair.0)               // the complete pair is initialized again
~~~

### 3.2 References, borrows, and provenance

`&T` is a shared reference and `&var T` is an exclusive mutable reference.
`&place` forms a shared borrow; `&var place` forms an exclusive borrow and
requires a mutable place. References are non-null. Field, tuple-field, method,
and call access automatically dereference references as required by the
selected operation.

Reference formation is explicit except for bound-method receiver adaptation and
the slice coercions in Section 4.1. A context expecting an ordinary `&T` or
`&var T` never silently borrows an argument or stored value.

While a shared borrow is live, the borrowed place may be read but cannot be
mutated, moved, dropped, or exclusively borrowed. While an exclusive borrow is
live, no overlapping shared or exclusive borrow and no other access to the
borrowed place is permitted. Access through the exclusive reference is the
only permitted access. A borrow ends after its last possible use, which may be
before the end of its lexical block.

A shared reference is `Copy`. An exclusive reference is move-only; using
`&var existing_reference` creates an exclusive reborrow for a shorter inferred
provenance rather than duplicating exclusive access. Converting `&var T` to
`&T` likewise creates a shared reborrow and does not consume the original
reference beyond the reborrow's live region.

Borrow checking operates on places. Disjoint struct and tuple fields may be
borrowed independently. Dynamic indices conservatively overlap; distinct
compile-time array indices may be treated as disjoint. Replacing an aggregate
overlaps every field within it, so an aggregate cannot be replaced while a
field borrow is live.

~~~elx
var point = Point { x: 0.0, y: 0.0 }
let view = &point
println(view.x)
// point.x = 1.0              // invalid while `view` remains live
println(view.y)

var left = Point { x: 0.0, y: 0.0 }
let edit = &var left.x
*edit = 1.0
// println(left.x)            // invalid before the last use of `edit`
println(*edit)
~~~

A reference operand must be an addressable place. Bindings, fields of
addressable values, array elements, and `Vec` elements are addressable.
Function results and computed expressions are not; code binds such a value
before borrowing it. Borrowing a `Vec` element prevents operations that may
move its backing storage. Map values are borrowed only through the standard
`get`/`get_var` APIs so lookup, table mutation, and absence remain explicit.
Map keys and set elements cannot be mutably borrowed.

References may occur in fields, enum payloads, tuples, slices, generic
arguments, associated types, closure environments, parameters, and returns.
Their inferred provenance is part of the checked type even though it has no
source spelling. Constructing an aggregate propagates every contained
provenance into the aggregate; projection recovers the corresponding
provenance. Moving, wrapping, erasing behind a trait object, or substituting a
generic type never erases a borrow.

A value containing a borrow cannot outlive any source represented by its
provenance. A function cannot return a borrow of a local, temporary, by-value
parameter storage, or value destroyed before return. A returned borrow is tied
to `self` when the return may borrow from `self`. Without such a receiver, it
is tied to the sole borrow-bearing input from which the result can be derived.
If more than one input could supply the returned provenance, the signature is
ambiguous and is rejected; the function must return an owned value or use a
wrapper whose API selects one source structurally. The compiler records these
relationships in package metadata so separately checked callers enforce the
same rule.

~~~elx
fn first(values: [i32]) -> &i32:
    return &values[0]         // result provenance comes from `values`

fn invalid() -> &i32:
    let value = 42
    return &value             // invalid: local storage cannot escape
~~~

Ordinary reference formation never promotes or allocates storage. A safe
reference to owned heap data keeps no independent ownership: the owning
`Box`, collection, `Shared`, store, or foreign wrapper must remain valid for
the complete borrow. Explicit ownership abstractions are specified in Section
9.

Raw pointers do not participate in borrow tracking after conversion. Creating a
raw pointer from a reference preserves its provenance and access rights, but a
raw pointer does not keep its source alive. Converting a raw pointer back to a
reference requires `unsafe` and asserts that the resulting borrow remains valid
for every safe use, as specified in Section 3.3.
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
instance later occupies the same address. For Elamite-owned storage, retaining
its `Box`, collection, `Shared`, store, or other owner for the complete access
is part of the liveness obligation. For foreign storage, the foreign contract
determines its lifetime and access rights.

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
concurrent-access requirements is undefined behavior. Later address reuse
cannot make a dangling raw pointer valid. An implementation may diagnose or
trap additional violations, but a program cannot rely on it doing so.

Converting a raw pointer to a safe reference asserts that all of the raw
pointer obligations will remain satisfied for every use of the resulting
reference. The reference receives inferred provenance but does not acquire
ownership or extend either Elamite or foreign storage. Unsafe code that
constructs it is responsible for choosing a region no longer than the owner's
actual validity. Safe code alone cannot create undefined behavior through a
raw pointer because it cannot dereference one or convert one to a reference. If
a later safe reference use observes a violated foreign lifetime contract, the
undefined behavior is attributable to the earlier unsafe construction.

## 4. Types

### 4.1 Primitive, tuple, string, slice, and collection types

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
type suffix. A separator occurs only between two digits of one run. An
unsuffixed integer materializes as an expected numeric type when representable
and otherwise defaults to `i32`. A floating literal uses an expected floating
type and otherwise defaults to `f64`. Unary `-` is an operator, but range
checking includes an immediately applied minus so each signed minimum is
expressible.

Concrete numeric types never convert implicitly. `value as Type` performs an
explicit conversion. Integer narrowing and float-to-integer conversion trap
when invalid; float-to-integer truncates toward zero. Integer-to-float and
float-to-float conversion use IEEE rounding. The standard library supplies
`try_from`, `wrapping_from`, and `saturating_from` where meaningful.

Integer arithmetic traps on overflow, division by zero, signed-minimum division
by `-1`, and invalid shifts in every build. Explicit checked, wrapping, and
saturating operations provide alternatives. Floating arithmetic follows IEEE
754. `isize` and `usize` use the selected target's pointer width.

Tuples use parentheses. `()` is unit and the empty tuple, `(value)` groups, and
`(value,)` is a one-element tuple. A zero-based positional selector such as
`pair.0` is an unsuffixed canonical decimal index statically within the tuple's
arity. It composes with other postfix operations and evaluates its receiver
once.

A tuple projection is a place when rooted in a place. In a consuming context it
moves a non-`Copy` component or copies a `Copy` component. It may be assigned,
borrowed, or exclusively borrowed when its root permits that operation.
Reference and raw-pointer receivers use the automatic dereference rules from
Sections 3.2 and 3.3.

`str` is an immutable UTF-8 view. A string literal has static provenance and
defaults to `str`; it materializes as an owned `String` when an expected
`String` type exists. `String` is the move-only, uniquely owned, growable UTF-8
string type. Moving a `String` transfers its buffer, `clone()` duplicates its
contents, and dropping it releases its buffer. Mutating one `String` can never
mutate another except through an explicit reference to the same owner.

An existing `str` does not implicitly allocate a `String`; use
`String.from(text)`. Borrowing a `String` as `str` produces a view whose
provenance is tied to that `String`. Operations that may reallocate the string
are invalid while such a view is live. `str` qualifies for `StableHash`;
`String` does not because its contents are mutable.

Ordinary string and character literals use double and single quotes. They may
contain Unicode scalar values directly. The supported escapes are `\\`, `\"`,
`\'`, `\n`, `\r`, `\t`, `\0`, and `\u{HEX}`, where `HEX` contains one through
six hexadecimal digits and denotes a valid Unicode scalar. A character literal
decodes to exactly one scalar. Invalid escapes, physical newlines, invalid
scalars, and unterminated literals are compile-time errors.

A fixed array type is `[T; N]`, where `N` is a compile-time nonnegative `usize`.
`[first, second]` constructs an array. An array owns its elements inline. It is
`Copy` exactly when `T` is `Copy`, and is `Clone` exactly when `T` is `Clone`.
A statically selected element may be partially moved under Section 3.1;
movement through a dynamic index is invalid unless the element is `Copy`.

A shared slice is `[T]`; an exclusive mutable slice is `[var T]`. Slices do not
own their elements. A shared slice is `Copy`; a mutable slice is move-only and
may be reborrowed. Both carry structural provenance and provide `len`, checked
indexing, and index-order iteration. In a context expecting `[T]`, an explicit
`&array` or `&vector` coerces to a shared slice. In a context expecting
`[var T]`, `&var array` or `&var vector` coerces to a mutable slice. A mutable
slice may reborrow as a shared slice. No coercion allocates.

The compiler-handled forms `@vec[...]`, `@map{key: value, ...}`, and
`@set{value, ...}` construct `Vec`, `Map`, and `Set`. Their elements evaluate
left-to-right. An empty literal needs an expected collection type. A later
duplicate map key replaces and drops the earlier value; duplicate set elements
collapse to one owned element.

`Vec[T]`, `Map[K, V]`, and `Set[T]` are move-only owning values. Each owns one
backing allocation or table identity. Moving transfers that ownership.
`clone()` creates an independent collection by cloning its contents and exists
only when the required element, key, and value types implement `Clone`.
Dropping a collection drops its contained values and releases its storage.
There is no copy-on-write or shallow mutable backing alias.

`Map[K, V]` keys and `Set[T]` elements require the compiler-controlled
`StableHash` capability. It is inferred structurally for immutable equality and
hashing: integral primitives, `bool`, `char`, `str`, `()`, and aggregates whose
participating fields qualify. `String`, collections, floating-point values, and
ordinary references do not qualify. `Identity[&T]` explicitly hashes target
identity when such keys are required.

Array, slice, and `Vec` indices have type `usize`; out-of-bounds access traps,
and a statically invalid array index is a compile-time error. Indexing produces
a place. Reading a `Copy` element copies it. A non-`Copy` element must be
borrowed, cloned explicitly, replaced, or removed through an ownership-taking
API. Assigning an element moves in the replacement and drops the previous
owned value.

Arrays provide `len`, `get(index) -> Option[&T]`, and
`get_var(index) -> Option[&var T]`. Slices provide the corresponding operations
permitted by their mutability. `Vec` provides:

~~~elx
fn len(self: &Self) -> usize
fn get(self: &Self, index: usize) -> Option[&T]
fn get_var(self: &var Self, index: usize) -> Option[&var T]
fn append(self: &var Self, value: T) -> ()
fn insert(self: &var Self, index: usize, value: T) -> ()
fn remove(self: &var Self, index: usize) -> T
fn pop(self: &var Self) -> Option[T]
fn clear(self: &var Self) -> ()
~~~

Insertion accepts zero through `len`, and removal requires an existing index.
Operations that may change a vector's length or capacity require exclusive
access to the vector and therefore cannot execute while any element or slice
borrow is live.

Map indexing is not an ownership-taking operation. It may copy a `Copy` value
and otherwise must be borrowed through `get` or `get_var`. Missing indexed
access traps. `Map` provides:

~~~elx
fn len(self: &Self) -> usize
fn contains_key(self: &Self, key: &K) -> bool
fn get(self: &Self, key: &K) -> Option[&V]
fn get_var(self: &var Self, key: &K) -> Option[&var V]
fn insert(self: &var Self, key: K, value: V) -> Option[V]
fn remove(self: &var Self, key: &K) -> Option[V]
fn clear(self: &var Self) -> ()
~~~

`Set` has no indexing operation. `contains` and `remove` borrow their query,
while `insert` consumes the inserted value:

~~~elx
fn len(self: &Self) -> usize
fn contains(self: &Self, value: &T) -> bool
fn insert(self: &var Self, value: T) -> bool
fn remove(self: &var Self, value: &T) -> bool
fn clear(self: &var Self) -> ()
~~~

Collection mutation, borrowing, and iteration are governed entirely by the
ordinary ownership and borrow rules. Safe code therefore has no iterator
invalidation undefined behavior.
### 4.2 Structs

`struct` declares an aggregate value type. Its body contains fields only.
Inherent methods are declared in one or more module-level `impl Type` blocks;
an inherent block never adds fields or changes representation. Fields and all
applicable inherent methods share one member namespace.

~~~elx
struct Session:
    active: bool
    name: String

impl Session:
    pub fn new(name: String) -> Self:
        return Self { active: true, name: name }

    fn stop(self: &var Self) -> ():
        self.active = false
~~~

Within a struct body, `Self` denotes the enclosing struct in field types.
Within an inherent implementation, `Self` denotes its complete target type. A plain
`self: Self` parameter consumes its receiver, moving it unless `Self` is
`Copy`. `self: &Self` and
`self: &var Self` receive shared and mutable references respectively.
`self: *Self` and `self: *var Self` receive const and mutable raw pointers
respectively. These five forms are the only permitted types for a parameter
named `self`; other pointer types and other parameterized types must use an
ordinary parameter name. The same receiver forms are available to trait
methods.

A bound call such as `value.method()` adapts only its receiver. If the method
expects `self: Self`, the receiver expression is evaluated exactly once and
consumed into `self`. The receiver need not be addressable. A reference receiver
cannot supply an owned `self: Self`; code explicitly clones the target when
that is intended.

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

Every cycle in the inline value-containment graph of structs and enums must
cross an explicit indirection type: a reference, raw pointer, `Box`, `Shared`,
`Weak`, `Store`/`Handle` edge, or owning collection such as `Vec`. Transparent
aliases and inline wrappers such as `Option[T]` do not break a cycle. The rule
makes recursive storage and identity visible in source types while permitting
ordinary owned trees such as `Vec[Node]`.

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
or hashing do not qualify initially. `Identity[&T]` provides `Eq`, `Hash`, and
`StableHash` using the referenced target's address for the borrow's valid
region, allowing identity-keyed maps and sets whose inferred provenance cannot
outlive their targets.

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
example `&fn(i32, ...String) -> ()`. Elamite lowers this form as a shared slice
argument rather than C's untyped variadic calling convention. The caller owns
the packed temporary for the complete call. The slice may be reborrowed within
the callee but cannot escape through a return, stored value, or closure; code
that needs to retain the arguments explicitly clones them into an owned
collection.

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

### 5.1 Explicit-capture closure objects

A closure expression creates a safe, first-class object with its own function
boundary and one anonymous nominal type. Every closure uses capture brackets;
a capture-free closure is `fn[](parameters):`, and captures precede the
parameter list:

~~~elx
let offset = 4
let add = fn[offset](value: i32) -> i32:
    return value + offset

var total = 0
let accumulate = fn[&var total as state](value: i32) -> i32:
    *state += value
    return *state
~~~

Closure parameters have explicit types and cannot be variadic. A closure has no
generic parameter list, cannot be declared `unsafe`, and never captures
implicitly. It may appear inside a generic declaration, after which ordinary
substitution makes its anonymous type concrete.

Every enclosing local used by the body occurs exactly once in the capture list.
Module declarations, imports, types, and named functions require no capture.
A capture may use `source as alias`; the alias is its only name in the body.
Aliases and parameters share one local namespace. The binding initialized by a
closure is not in scope within that initializer, so anonymous recursion is
invalid.

Captures evaluate once from left to right when execution reaches the closure:

- `value` copies a `Copy` value and otherwise moves it into the closure;
- `&value` captures a shared borrow;
- `&var value` captures an exclusive borrow and requires a mutable place.

Raw pointers use the plain form because their value is already explicit and
`Copy`; capture does not dereference or retain the pointee. Later raw access
retains every `unsafe` requirement from Section 3.3.

The environment is stored inline in the closure object. Constructing a closure
does not allocate. Moving it moves its captured fields; it is `Copy` only when
every capture is `Copy`, implements `Clone` only when every capture can be
cloned without violating exclusivity, and runs ordinary field destruction.
Borrow captures propagate their provenance through the closure, so a closure
cannot escape any captured source.

Capture bindings cannot be rebound or moved out by the body. Mutation of
external state is explicit through a captured `&var` or another type whose API
provides interior synchronization. This permits one uniform call contract:
every closure implements `Callable[Arguments, Return]` and invocation borrows
the closure shared for the call. `Arguments` is the exact argument tuple.
Ordinary call syntax invokes the object repeatedly without consuming it.

The return annotation is optional. Explicit `return` expressions and an
expected callable result constrain one exact inferred type; reachable
fallthrough contributes `()`. There is no implicit tail-expression return.
An annotated non-unit closure returns on every normally completing path, and
`-> !` follows the ordinary never-return rules.

A generic function accepts a closure through a `Callable[Arguments, Return]`
bound. Borrowed erasure uses `&Callable[Arguments, Return]`; owning erasure uses
an explicit `Box[Callable[Arguments, Return]]`. Erasure never occurs
implicitly and allocation occurs only for the explicit owning box.

A capture-free `fn[]` closure may explicitly convert to an exact safe function
reference because it has no environment. That reference may then explicitly
convert to the corresponding raw function pointer under the ordinary function
rules. A capturing closure never converts to a function reference or C
callback; stateful C callbacks use a named or capture-free callback plus a
separate raw context pointer.

A closure does not inherit an enclosing `unsafe:`, loop, `defer`, or return
context. Its body starts safe and has its own `return`, postfix `?`,
never-return, and cleanup behavior. It cannot redirect an enclosing loop.
Initialized capture expressions, generic or variadic closure literals, unsafe
closures, implicit captures, trailing-closure statement syntax, anonymous
recursion, and callable equality or hashing are not supported.

A function reference is an ordinary storable value. It may appear in a binding,
field, enum payload, collection element, parameter, or return value. Named
functions, instantiated generic functions, and unbound methods produce function
references that are compatible only when parameter types, return type, arity, and
any variadic marker and safety qualifier match exactly. A safe function
reference does not convert implicitly to an unsafe function reference, and an
unsafe function reference never converts to a safe one. Function types have no
variance or implicit signature adaptation, and collections of them are
homogeneous by complete function type.

A named function has a stable address for the whole program. Its safe or unsafe
function reference is `Copy`, has no inferred provenance, and carries no
captured environment.

A generic function becomes a function reference only after all of its type
arguments are determined explicitly or by an expected function type. Elamite
has no runtime signature inspection or implicit signature erasure. A function
reference participates in exact `Callable` bounds, and explicit borrowed or
boxed `Callable` erasure follows Section 5.1.

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

Because a function reference carries no state, a callback that carries data
uses an explicit-capture closure or a trait object. A trait-object callback
stores the state in a struct and dispatches through `&Trait` (Section 6).

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
bound position even though users cannot implement it. `Copy`, `Send`, and
`Sync` are likewise compiler-controlled structural capabilities. `Clone` and
`Drop` are ordinary coherent traits with the special invocation rules in
Sections 3.1 and 8. A type cannot be both `Copy` and `Drop`. Elamite initially
has no `where` clauses, default type arguments, const generics, associated
types, or higher-kinded type parameters.

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
Trait objects provide dynamic dispatch. A borrowed trait object is `&Trait` or
`&var Trait` and carries the target borrow's provenance plus a static vtable.
An owning trait object is `Box[Trait]` and stores one concrete implementation in
an explicit heap allocation with the same vtable metadata.

A trait has no value representation, so a trait name denotes a type only as the
target of a safe reference or `Box`, as a generic or implementation bound, or
as the trait of an `impl Trait for Type`. A bare trait name in any other type
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

An owned `Box[T]` explicitly converts to `Box[Trait]` when `T` implements the
object-safe trait. The conversion consumes the source box, preserves its
allocation, and installs the vtable; it never clones the target or allocates a
second object.

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
borrowed or owning trait object. A generic trait can form an object only after all of its
trait type arguments are concrete. Default methods participate in the vtable.

Trait-object calls dispatch through the vtable, and different concrete target
types may coexist in a homogeneous collection such as `Vec[&Trait]` or
`Vec[Box[Trait]]`, using explicit borrowed or owning erasure.
Trait objects initially provide no downcasting, runtime concrete-type
inspection, or multi-trait object composition. Borrowed objects retain
structural provenance; owning objects retain ordinary `Box` ownership.

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

An inherent implementation has the form `impl Type:` or
`impl[T: Bound] Type:`. Its outermost canonical target must be a nominal type
declared in the same module as the block. Aliases are compared through their
canonical targets. Every implementation generic parameter must occur in the
target type, so the target determines all block substitutions. Methods may
declare their own generic parameters and their own `pub` visibility.

Several inherent blocks may apply to one type. A field and an inherent method
may not share a name. Two inherent blocks may declare the same method name only
when their canonical target patterns are provably disjoint. Generic parameters
are conservatively able to match any type, and bounds do not prove target
disjointness. Consequently an exact block does not override an overlapping
generic block; the overlap is invalid. These rules do not specialize trait
implementations.

Within a trait declaration, `Self` denotes the type that implements the trait.
Within either `impl Type` or `impl Trait for Type`, `Self` denotes the complete
implementation target `Type`. `Self` is invalid outside a struct body, trait
declaration, or implementation.

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
contribute to exhaustiveness. Pattern bindings move non-`Copy` payloads and
copy `Copy` payloads under Section 3.1. Matching a reference does not
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

Binary `++` concatenates values left-to-right. Text operands may be `str` or
`String` in either combination and produce an owned `String`; owned `String`
operands are consumed and their storage may be reused. Sequence-like owning
standard-library values are consumed and produce the same owning type.
Compile-time AST list concatenation follows Section 12.2. `++` is distinct from
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

### 7.1 Iteration

The standard `Iterator[Element]` trait declares
`fn next(self: &var Self) -> Option[Element]`. A user iterator expression is
evaluated once and moved into mutable hidden loop state. Each iteration
exclusively borrows that state for `next`; `Some(value)` moves the returned
element into the non-rebindable loop binding and `None` exits. `continue`
requests another element, and `break` exits without another call.

Hidden iterator state uses ordinary stack or inline storage and does not
allocate merely because `next` borrows it. If an iterator contains a borrow or
returns an element borrowing from itself, structural provenance prevents the
state or yielded value from escaping its source. A yielded borrow remains live
for its uses in that iteration and must end before the next exclusive `next`
call.

Arrays, slices, `Vec`, `Map`, and `Set` also support direct iteration:

- iterating an owned array or `Vec` consumes it and moves out elements in index
  order;
- iterating `[T]` yields `&T`, and iterating `[var T]` yields sequential
  `&var T` reborrows;
- iterating an owned map consumes it and yields owned `(K, V)` pairs; a shared
  map borrow yields `(&K, &V)`, and an exclusive map borrow yields
  `(&K, &var V)`;
- iterating an owned set consumes it and yields owned elements; a shared set
  borrow yields `&T`.

Map and set order is unspecified. Ownership and borrowing make invalidation a
static question: structural mutation cannot occur while an iterator or yielded
borrow conflicts with it. Direct iteration therefore introduces no safe-code
undefined behavior. Trait-object iteration and an implicit `IntoIterator`
conversion are not part of the initial owned model.
### 7.2 Formatted strings and display

`Display` is a compiler-recognized prelude trait with a formatting method that
writes a value to a mutable standard-library `Formatter`. Users may implement
it normally. Primitive values, `str`, `String`, references to displayable
values, and standard collections of displayable values provide implementations.
Its required method is
`fn fmt(self: &Self, formatter: &var Formatter) -> ()`. The initial formatter
surface provides `formatter.write(text: str) -> ()`; implementations compose
other values by writing a formatted string. A formatter uniquely owns its
buffer and releases it through ordinary destruction.

A formatted string literal uses the prefix `f`, as in
`f"point: {point.x}, {point.y}"`, and produces an owned `String`. Each
`{expression}` is evaluated exactly once in left-to-right order and its value
must implement `Display`. `{{` and `}}` produce literal braces. Unmatched braces
are compile-time errors. Elamite initially has no width, precision, positional,
or debug-format specifiers. Braces in an ordinary string literal have no
formatting behavior.

The prelude `print` and `println` functions each take one generic `Display`
value. Passing a non-`Copy` value consumes it; passing `&value` displays it
without consumption because references to displayable values implement
`Display`. They are ordinary generic functions rather than special
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
moves its payload into the postfix expression unless it is `Copy`.
`Err(error)` moves the error into `Result.Err(error)` and immediately returns
from the enclosing function after ordinary cleanup of every exited scope.

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
lexical, typing, trait, safety, FFI, control-flow, ownership, and cleanup rule.
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
    WrongStore
    StaleHandle

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
`E-RUN-INDEX`, `E-RUN-KEY`, `E-RUN-CAST`, `E-RUN-NULL`, `E-RUN-ALIGN`,
`E-RUN-CLOSED`, `E-RUN-STORE`, and `E-RUN-STALE`, respectively. Allocation
failure is deliberately absent:
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

Owned values are destroyed deterministically. `Drop` is the one
compiler-recognized cleanup trait:

~~~elx
trait Drop:
    fn drop(self: &var Self) -> ()
~~~

A user may implement `Drop` for a local nominal type under the ordinary
coherence rules. Its method is safe, non-generic, unit-returning, and cannot be
called directly. The compiler invokes it exactly once for each initialized
owned value that is not moved away. A `Drop` type is never `Copy` and cannot be
partially moved.

On ordinary scope exit, initialized locals are destroyed in reverse successful
initialization order. For a value with a `Drop` implementation, its `drop`
method runs first and then its still-initialized fields are destroyed in reverse
declaration order. Types without custom `Drop` use structural field
destruction. Replacing an initialized `var` evaluates the new value first,
destroys the old value, and then installs the replacement. The prelude
`drop(value)` consumes a value and ends its ownership at that call.

Cleanup runs on fallthrough, `return`, postfix `?`, `break`, and `continue`.
The returned or propagated value is evaluated and moved to caller-owned result
storage before cleanup begins. Process-fatal panic, typed traps, foreign
crashes, signals, and OOM do not unwind and do not guarantee any remaining
cleanup.

`Drop.drop` cannot report a recoverable error. A resource that needs fallible
flush, commit, or close behavior exposes a separate explicit method returning
`Result`; callers handle that operation before scope exit. Its `Drop`
implementation provides only the documented infallible fallback release.

`defer` remains the explicit mechanism for actions that are not ownership
destruction. `defer call` registers one safe unit-returning function or method
call. `defer:` registers one indented statement block. Registration occurs only
when execution reaches the statement. A deferred action is not a closure or
first-class value and cannot escape its block.

A deferred call is evaluated at scope exit, not registration. Its referenced
bindings must therefore remain initialized and valid until then; a later move,
conflicting borrow, or destruction that would invalidate the call is rejected.
Reassigning an otherwise available `var` changes the value observed by the
deferred call.

For one scope, deferred actions execute first in reverse registration order,
then automatic local destruction executes in reverse initialization order.
Inner scopes finish all deferred actions and destruction before an outer scope
continues cleanup. This fixed ordering keeps every binding used by a deferred
action alive until the action completes.

~~~elx
let file = File.open("report.txt", "w")?
defer file.flush_report()

let left = Buffer.new()
let right = Buffer.new()
defer:
    left.record_metrics()
    right.record_metrics()

file.write("Elamite report")?
~~~

A deferred block is an ordinary lexical scope but cannot redirect control while
its enclosing scope is exiting. `return`, `break`, `continue`, postfix `?`, and
nested `defer` are invalid inside it. A `defer` statement is invalid inside an
`unsafe:` block, an `unsafe:` block is invalid inside `defer:`, and a direct
unsafe or foreign call cannot be deferred. A safe wrapper may establish and
encapsulate any native cleanup contract.

Elamite has no `errdefer`. Conditional cleanup uses ordinary state in a
deferred safe wrapper or explicit control flow. Automatic destruction is not
conditional on success versus error propagation.
## 9. Explicit memory ownership

Elamite has no tracing garbage collector and no implicit promotion of
address-taken storage. Ordinary locals, temporaries, aggregates, and closure
environments use inline or stack storage according to their lexical ownership.
Heap storage is introduced only by an owning type or an operation whose API
documents allocation.

`Box[T]` uniquely owns one address-stable heap allocation. Moving a box moves
the owning handle without moving `T`; borrowing a box borrows its pointee;
dropping it drops `T` and releases the allocation. `Box[&Trait]` is unnecessary:
owning trait erasure is represented by `Box[Trait]`, whose allocation stores the
concrete value and its dispatch metadata.

`Shared[T]` provides explicit shared ownership through an atomic strong count.
Cloning a `Shared` increments the count; dropping one decrements it; the last
strong owner destroys `T`. `Shared` exposes shared borrowing only and does not
make mutation safe. Shared mutable state uses an API that enforces its access
contract, normally `Shared[Mutex[T]]` or an atomic type.

`Weak[T]` is a non-owning companion to `Shared[T]`. Downgrading does not change
the strong count; `upgrade() -> Option[Shared[T]]` succeeds only while a strong
owner exists. Weak bookkeeping storage may remain until the last weak value is
dropped. A cycle made entirely of `Shared` strong edges is not reclaimed; code
uses `Weak` for back edges or a checked store for graph ownership.

`Shared[T].new(value)` constructs one strong owner, `owner.get()` borrows its
value, and `owner.downgrade()` constructs a weak owner. `weak.upgrade()` has the
`Option[Shared[T]]` result described above. `Shared` and `Weak` equality is
control-block identity rather than structural equality of `T`.

`Store[T]` owns a homogeneous table and returns opaque `Handle[T]` identities.
A handle is `Copy` and logically contains a store identity, slot, and generation
even when the implementation packs them differently by target. Looking up a
handle requires the corresponding `&Store[T]` or `&var Store[T]`. A wrong-store
or stale-generation lookup traps; removal increments the generation before a
slot can be reused. A handle neither owns nor borrows an element and cannot be
dereferenced without its store.

Shared store lookup returns `&T`; exclusive lookup returns `&var T`. Invalid
handles trap rather than encoding programmer-controlled absence. Insertion,
removal, compaction, and any operation that may
relocate elements require exclusive store access and conflict with live element
borrows. Dropping a store destroys every remaining element regardless of cycles
formed by handles. This makes `Store` the standard ownership model for mutable
graphs whose identity should not retain nodes individually.

`Store[T].new()` constructs an empty store. `insert(value) -> Handle[T]`,
`get(handle) -> &T`, `get_var(handle) -> &var T`, and `remove(handle) -> T`
provide the core graph operations; `len`, `is_empty`, `clear`, and `compact`
have their ordinary collection meanings. Direct `for` iteration consumes the
store and moves each remaining `T` in insertion order; breaking destroys every
unvisited element. Iteration therefore cannot leave handles naming a live
store behind.

Owned collections, `String`, boxes, shared allocations, stores, formatting, and
explicit owning erasure may allocate. Borrow formation, receiver adaptation,
slice coercion, closure construction, and iterator hidden state do not allocate
by themselves. The non-normative `cost_model.md` records current implementation
costs without weakening these semantic boundaries.

Allocation failure is process-fatal. OOM is not a `Result`, cannot be caught,
and does not unwind or guarantee cleanup. A successful safe allocation is
properly aligned and never yields `null`.

Moving an owning handle cannot invalidate borrows into its separately allocated
pointee, but destroying that owner or performing an operation documented to
relocate backing storage conflicts with those borrows and is rejected. Raw
pointers never retain an owner. Safe references carry provenance but no
ownership of their own.

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
unsafe pub fn from_pointer(owner: &Session, pointer: *Session) -> &Session:
    unsafe:
        return pointer as &Session
~~~

The returned reference above receives its public provenance from `owner`; the
unsafe implementation promises that `pointer` identifies storage valid for no
less than that borrow. A function cannot return an unbounded safe reference
from only a raw pointer.

Using an unsafe-only operation outside the unsafe context required by Section
3.3 is a compile-time error. The expression-local constant rule in Section 3.3
is the only mandatory value analysis for raw-pointer access. A compiler may
warn when broader analysis indicates a violation of provenance, liveness,
bounds, initialization, pointee-type, or write-permission obligations, but
inability to prove valid foreign input is not itself an error. Safe-reference
escape remains governed by inferred provenance and is never repaired by hidden
allocation.

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
target. It follows ordinary ownership rules and is `Copy` only when every field
is `Copy`; its fields must themselves be ABI-safe and it cannot be generic,
derive traits, contain methods, implement `Drop`, or contain an incomplete
opaque type directly. The declaration author is responsible for matching the C
header exactly. A mismatched declaration is an unsafe contract violation.

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
add any required terminator, pass a raw pointer and length, and retain the
owning storage for as long as foreign code may access it.

Every imported foreign function is unsafe to call, including one whose
signature contains only scalars. Its declaration has no body, cannot use
Elamite's homogeneous variadic parameter syntax, and its value has the
corresponding raw function-pointer type. C variadic functions are not supported
in the initial interface. Each raw-pointer parameter and result must have a
documented foreign contract covering nullability, readable or writable extent,
alignment, pointee initialization and type, whether the pointer is retained,
and what event ends its validity. The compiler does not infer those facts from
a C header. C output parameters use explicit `std.ffi.MaybeUninit[T]` storage;
unsafe wrapper code verifies successful initialization before converting it to
an owned `T`. Reading uninitialized storage is undefined behavior.
`MaybeUninit[T].new()` creates logically uninitialized, ABI-aligned storage for
an ABI-safe `T`; `pointer()` requires a mutable storage place and returns
`*var T`; and unsafe `assume_init()` consumes the storage to produce `T` after
the wrapper has verified the foreign success condition. `MaybeUninit` is
move-only and does not destroy a logically uninitialized `T`.

### 10.2 Ownership, retention, and foreign resources

A raw pointer never owns its target and does not transfer ownership by itself.
Every owning C contract is represented by an Elamite wrapper that is move-only
and implements `Drop` with the matching native release operation. If the C API
permits an explicit fallible close, the wrapper exposes that operation
separately and leaves `Drop` as an infallible fallback. Shared native identity
uses an explicit `Shared` wrapper rather than copying an owning raw handle.

Safe references, slices, owning Elamite values, and closure objects are not
ABI-safe. A wrapper explicitly converts a borrow to a raw pointer. For a
non-retaining call, the borrow remains live for the complete call and prevents
conflicting movement or mutation.

When C retains a pointer after return, the pointed-to value must have
address-stable explicit ownership, normally `Box[T]` or `Shared[T]`. The wrapper
that registered the pointer keeps that owner until the foreign API confirms
unregistration. Moving the owner handle is permitted because the allocation
does not move; dropping it, mutating through conflicting aliases, or allowing a
collection to reallocate before unregistration violates the unsafe contract.

When C transfers an allocation to Elamite, its wrapper stores the raw pointer
and exact allocator-compatible deleter. When Elamite transfers ownership to C,
the consuming wrapper operation suppresses Elamite destruction only after C has
accepted ownership. Allocator pairing is part of the unsafe contract; Elamite
never assumes that C `free`, an Elamite allocator, and a library-specific
release function are interchangeable.

For an Elamite `Box[T]`, `pointer()` and `pointer_var()` expose nonowning raw
addresses while the box remains the owner. Consuming `into_raw()` transfers
cleanup responsibility with a `*var T`. Unsafe `Box[T].from_raw()` restores
that owner only when the pointer is the live result of the compatible Elamite
box allocation contract and has not already been adopted or freed. A pointer
allocated by a foreign library instead belongs in that library's ordinary
move-only wrapper with its exact deleter; it is not converted to `Box` merely
because both implementations currently use a C allocator.

A raw pointer returned by C does not retain foreign storage. Converting it to a
safe reference requires an inferred provenance source that is no longer than
the foreign contract, typically an owner or session borrow passed alongside the
pointer. Returning a safe reference with no such source is invalid even inside
`unsafe`.
### 10.3 Callbacks and foreign control flow

A module-level function may use `@exportc("c_name")` to emit one exact unmangled
C symbol. Its signature must be ABI-safe. The declaration remains an ordinary
safe or unsafe Elamite function, and unsafe-only operations in its body still
require explicit `unsafe:` blocks.

~~~elx
@exportc("visit_value")
unsafe fn visit_value(context: *var i32) -> i32:
    unsafe:
        let value = context as &var i32
        *value += 1
        return *value
~~~

A named function or capture-free `fn[]` closure with an ABI-safe signature may
explicitly convert to a matching raw function pointer. `@exportc` is required
only when C links to a symbol by name. Capturing closure objects never cross as
function pointers.

Stateful callbacks use a raw context pointer. A registration wrapper owns the
address-stable `Box` or `Shared` state, passes its raw pointee address, and keeps
the owner until C has released both callback and context. The callback recovers
a reference only inside `unsafe` and only for the region justified by that
owner. Destruction unregisters before releasing the context.

Foreign-created threads may enter Elamite only through an exported function or
registered callback address. The runtime establishes its per-thread execution
state on entry; no garbage-collector attachment exists. The unsafe registration
contract must guarantee that concurrently accessed context satisfies the same
`Send`, `Sync`, synchronization, and lifetime obligations as an
Elamite-created thread. A wrapper cannot make unsynchronized foreign mutation
safe merely by hiding it.

Recoverable Elamite errors do not cross the ABI automatically. A wrapper
translates them to status codes, initialized out-parameters, or another
documented C representation. `errno` and foreign error channels are observed
only explicitly. An Elamite trap during a foreign call or callback terminates
the process and never unwinds through C. C++ exceptions, `longjmp`, or any
foreign unwinding across an Elamite frame are forbidden; foreign code contains
and translates them before returning.
### 10.4 Native threads and race-safe synchronization

Elamite exposes native threads through ordinary `std.thread` and `std.sync`
declarations. It adds no concurrency syntax, `async`, or implicit task runtime.

`Send` and `Sync` are compiler-controlled capabilities. `Send` means an owned
value may move to another thread; `Sync` means shared references to a value may
be used from multiple threads. They are derived structurally. `&T` is `Send`
when `T` is `Sync`; an exclusive borrow may enter a scoped thread when its target
is `Send`. Raw pointers are neither capability automatically. An FFI wrapper
may assert a capability only with an explicit unsafe implementation whose
author owns the complete synchronization contract.

`std.thread.spawn` evaluates and consumes one zero-argument closure object. The
closure and result must be `Send`, and its environment may contain no
non-static borrow. Creation starts one native thread eagerly and returns
`Result[Thread[R], SpawnError]`; OS creation failure is recoverable and OOM is
fatal. The thread calls its moved inline closure without copying or detaching
captured state.

`Thread[R]` is move-only. `join(self: Self) -> R` consumes the handle, waits
once, and moves out the result. Dropping an unjoined handle detaches that handle
but does not cancel the thread; a normally exiting process waits for all
Elamite-created threads and drops unclaimed results. Self-join traps and general
join cycles may deadlock. There is initially no cancellation or interruption.

Borrowing parallelism uses the ordinary function `std.thread.scope`. It accepts
a closure whose `Scope` argument can spawn child closures borrowing from the
scope's inferred region. A scoped child cannot outlive the call, and neither its
handle nor a result containing a scoped borrow may escape. Explicit joins
consume scoped handles; scope exit joins any remaining children before
returning. No `scoped` keyword or alternate closure syntax exists.

~~~elx
var values = [1, 2, 3]
std.thread.scope(fn[&var values](scope: &var std.thread.Scope) -> ():
    let worker = scope.spawn(fn[&var values]() -> ():
        values[0] = 4
    )
    worker.join()
)
println(values[0])
~~~

Channels move messages. `channel[T: Send]` returns move-only endpoint values;
cloning an endpoint explicitly creates another synchronized endpoint identity.
`send(value: T)` consumes `value`, returning it inside the failure value if the
channel is closed. Receive moves one queued message to the receiver. Dropping
the final sender closes the channel deterministically; buffered messages remain
receivable before `None`. Capacity zero is rendezvous, bounded capacity applies
backpressure, and an explicitly named unbounded constructor may allocate.

`Mutex[T]` owns one `T`. `lock(self: &Self)` returns a move-only guard that
borrows the mutex; `guard.get()` returns `&T` and `guard.get_var()` returns
`&var T`. Destroying the guard unlocks.
Protected data cannot be accessed without a live guard, and the guard cannot
outlive the mutex. Shared mutable state is normally
`Shared[Mutex[T]]`. Process-fatal traps do not unwind and therefore do not
promise guard destruction; the runtime terminates rather than exposing a
poisoned surviving process.

`AtomicBool`, `AtomicI32`, and `AtomicUsize` are non-`Copy` atomic cells.
Their operations borrow the cell and are sequentially consistent. Sharing an
atomic uses a shared reference or `Shared`; copying an atomic scalar value does
not copy the cell identity. The C99 backend implements atomics through
runtime/compiler hooks rather than C11 `_Atomic`, including target-width
`usize` operations on x86.

Safe code cannot create two conflicting unsynchronized cross-thread accesses:
ownership, borrow provenance, and `Send`/`Sync` prevent the shared mutable
alias. Data races remain possible only after an unsafe capability assertion,
raw-pointer operation, or violated foreign contract, and then constitute
undefined behavior under the raw/foreign contract rather than ordinary safe
semantics.

Thread start orders prior initialization before the child begins. Mutex unlock
orders guarded writes before a later successful lock. A successful send orders
message initialization and prior evaluations before the matching receive.
Thread completion orders prior evaluations before `join` returns.
Sequentially-consistent atomics participate in one total order. These edges
compose; moving or cloning an ordinary value creates no synchronization edge.

A thread body is its own safe function boundary with ordinary `return`, `?`,
`defer`, and destruction on normal exit. A panic, typed trap, foreign crash, or
OOM on any thread terminates the process and is never converted to a join
result. Scheduling, fairness, relative completion, and output-call order are
unspecified; complete standard output calls are internally synchronized.

Executors, futures, `async`/`await`, detached-process lifetime guarantees,
cancellation, interruption, timeouts, relaxed atomics, parallel iterators,
fairness guarantees, and general deadlock detection are outside this contract.
## 11. Conformance example

~~~elx
struct Counter:
    value: i32

impl Counter:
    fn increment(self: &var Self) -> ():
        self.value += 1

fn main() -> Result[(), String]:
    var counter = Counter { value: 0 }
    let counter_ref = &var counter
    counter_ref.increment()
    println(counter_ref.value)

    // The exclusive borrow ended after its last use.
    let moved_counter = counter
    println(moved_counter.value)

    let name = String.from("Elamite")
    let independent = name.clone()
    let describe = fn[name]() -> String:
        return f"hello {&name}"

    println(describe())
    println(independent)
    return Result.Ok(())
~~~

The complete normative surface is demonstrated by the executable
[`examples/spec_demo.elx`](../examples/spec_demo.elx) and the focused
conformance suites indexed by [ledger.md](ledger.md).
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
  `Implementation`, `InherentImplementation`, and `FieldDefinition`;
- `Expression`, `StatementList`, `ItemList`, `MemberList`, `Pattern`, and
  `TypeSyntax`; and
- `Identifier` plus opaque origin information suitable for diagnostics.

The model represents pre-resolution structural syntax: written names and type
syntax, visibility, generic syntax, fields, variants, parameters, bodies,
documentation, attributes, and provenance. It does not expose inferred types,
resolved trait selection, layouts, target properties, runtime values, mutable
compiler tables, or arbitrary compiler internals. The current exact interface
version is 2.0. Version 1.0 remains frozen and is not source- or artifact-
compatible: an artifact requiring it receives the exact version-skew
diagnostic. In 2.0 `StructDefinition` contains fields only, while
`InherentImplementation` carries a target type and method members. A later read-only semantic
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
let fields: std.ast.MemberList = quote:
    id: u64

let behavior: std.ast.InherentImplementation = quote:
    impl Entity:
        pub fn identifier(self: &Self) -> u64:
            return self.id

let all_fields = target.members() ++ fields
let result = target.with_members(all_fields)
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
change fields, variants, visibility, documentation, and ordinary attributes,
and may emit behavior as sibling items, but cannot bypass privacy or any later
semantic check.

`StructDefinition` is field-only in `std.ast` 2.0. An attribute that adds
inherent behavior returns an `ItemList` containing the transformed definition
and sibling `InherentImplementation` items. Returning a same-kind definition
can change only structure carried by that definition.

~~~elx
attr identifiable(
    target: std.ast.StructDefinition,
) -> std.ast.ItemList:
    let fields: std.ast.MemberList = quote:
        id: u64

    let target_type = target.type_syntax()
    let behavior: std.ast.InherentImplementation = quote:
        impl $target_type:
            pub fn identifier(self: &Self) -> u64:
                return self.id

    let definition = target.with_members(target.members() ++ fields)
    return std.ast.items(definition, behavior)

@attr(identifiable)
struct Entity:
    name: String
~~~

### 12.6 Derives

A derive declaration has exactly one target parameter, typed as
`std.ast.StructDefinition` or `std.ast.EnumDefinition`, and returns
`std.ast.Implementation`. `@derive(Name, ...)` retains the original definition
and invokes the selected generators in source order. All ordinary attributes on
that definition finish first, so derives observe the final fields, variants,
metadata, and attributes of that definition. Sibling items emitted by an
attribute, including inherent implementations, are scheduled independently and
are not observed as members of the derive target.

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
