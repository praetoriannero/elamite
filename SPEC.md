# Elamite Language Specification

> Status: Draft
>
> Version: 0.3.0-draft
>
> The current [specification demonstration](examples/spec_demo.elx) is the
> authoritative surface-language example. This document describes that design;
> ambiguities and internal inconsistencies that still need decisions are listed
> in [ISSUES.md](ISSUES.md).

## 1. Overview

Elamite is a statically typed, garbage-collected language that compiles to C.
It provides value types, explicit references, traits, generic types, algebraic
data types, recoverable errors, raw pointers behind an unsafe boundary, and
indentation-delimited control flow.

Ordinary values are passed and assigned by logical value copy. Each copy is
observably independent except where the copied type explicitly represents an
alias or identity, such as a safe or raw reference, a function or closure
handle, a trait-object reference, or a shared resource handle. Passing
`&value` explicitly passes a shared reference; passing `&var value` explicitly
passes a mutable reference. Elamite has no source lifetime parameters. Managed
memory uses Boehm GC; programs should not use collection timing for resource
cleanup.

## 2. Program layout

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

This body form is used for `mod`, `struct`, `enum`, `trait`, `impl`, `if`,
`else`, `match`, `for`, `while`, `with`, `unsafe`, anonymous closures, and
function declarations with bodies. Brace-delimited and same-line bodies are
invalid. An empty body is also invalid; `pass` is the explicit no-op statement
when a body must otherwise be empty.

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
either `library` or `executable`. `src/lib.elx` and `src/main.elx` are the
respective default roots; the manifest may select a different `.elx` file. The
directory containing the selected root file is the package's source directory.

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
module is an error. `std` names the standard-library package. Each dependency's
manifest alias begins a path into that dependency. An unqualified name is found
only among lexical bindings, declarations and imports in the current module,
and prelude names; lookup never searches unrelated modules.

`import path` is permitted at module level, including within an inline module,
and binds the final path component in that module. `import path as name` uses an
explicit local alias. Imports are not inherited by nested modules. Wildcard and
grouped imports are initially unsupported. Import order has no semantic effect.

Declarations are package-private unless prefixed with `pub`: every module in
the defining package may access a package-private declaration, but dependent
packages may not. `pub` applies to modules, functions, structs, enums, traits,
and type aliases. Fields and inherent methods are package-private unless
individually marked `pub`. All variants and variant payload fields of a public
enum are public. All methods of a public trait are public as defined in Section
6.

`pub import path` and `pub import path as name` re-export a public declaration
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
module-item names.

Circular imports between modules of one package are permitted. The compiler
collects module declarations before resolving imports and bodies. Imports
execute no code and establish no runtime initialization order. The package
dependency graph must be acyclic.

A package has an opaque identity determined by the dependency instance resolved
from its manifest and lockfile, not merely by its displayed name. Packages from
different versions or sources have distinct identities and therefore distinct
nominal types and traits. A dependency alias does not change that identity.
The package identity used by these rules is the package identity used by trait
coherence and the orphan rule in Section 6.

~~~elx
import std.io
import root.models.User as InternalUser
import root.codec.json as json

pub mod diagnostics:
    import std.io
    import super.UserId

    pub fn report(message: String):
        io.println(message)

pub type UserId = u64
pub import root.models.User
pub import root.codec as codec
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

Assignment, ordinary argument passing, and ordinary returns copy the source
value; using the source after that operation is valid. Copying is a core
property of every value and is not controlled by a trait.

An ordinary copy is recursively and observably independent. Mutating any
ordinary nested field, string, array, or collection in one copy does not change
another copy. An implementation may share immutable or copy-on-write backing
storage, but such sharing cannot be observed through language operations.

Types that explicitly carry aliasing or identity retain that meaning when
copied. Safe references, raw pointers, trait-object references, and closure
handles continue to identify the same target or closure environment. A type
implementing `Close` follows the shared-resource-handle contract in Section 8.
Copying a containing aggregate preserves these explicit aliases while all of
its ordinary value fields remain independent.

~~~elx
struct Address:
    city: String

struct User:
    name: String
    address: Address

let original = User { name: "Ari", address: Address { city: "Aster" } }
var changed = original
changed.name = "Bea"
changed.address.city = "Beacon"

println(original.name)         // "Ari"
println(original.address.city) // "Aster"

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
instead returns an ordinary independent value copy.

An array or `Vec` element and a `Map` value may still be an assignable place
when reached through a mutable collection path. Replacement, compound
assignment, and direct nested-field mutation update that collection value, but
no reference to the selected interior may escape. `Map` keys and `Set` elements
are never mutable places; changing one requires removing it and inserting a new
value. These rules make collection backing-storage strategy unobservable.

~~~elx
var points = @vec[Point { x: 0.0, y: 0.0 }]
let original_points = points
var first = points[0]

first.x = 0.5
points[0].x = 1.0

println(first.x)              // 0.5
println(points[0].x)          // 1.0
println(original_points[0].x) // 0.0

// Invalid: collection interiors cannot be referenced.
// let first_ref = &var points[0]
~~~

References are valid struct fields, enum payloads, collection elements, closure
captures, parameter types, and return types. A reference formed through safe
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

A reference path that enters a nested aggregate targets that selected
subvalue rather than rebasing through a later replacement of its containing
value.

~~~elx
var user = User {
    name: "Ari",
    address: Address { city: "Aster" },
}
let city: &String = &user.address.city

user = User {
    name: "Bea",
    address: Address { city: "Beacon" },
}
println(city) // "Aster"
~~~

### 3.3 Raw pointers and null

Raw pointer types are `*T` and `*var T`. A raw pointer can be `null`; `&T` and
`&var T` are always non-null. A nullable safe reference is represented by
`Option[&T]` or `Option[&var T]`. Conditions require `bool`, so neither raw
pointers nor references have implicit truthiness. Code tests a raw pointer with
an explicit comparison such as `pointer == null`.

`&T` may convert safely to `*T`; `&var T` may convert safely to `*var T`.
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

Pointer provenance, the consequence of violating unsafe pointer obligations,
and the compiler's treatment of unsafe references remain open in
[I-020](ISSUES.md#i-020-raw-pointer-provenance-and-violations).

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
type suffix. An unsuffixed integer literal materializes as an expected integer
or floating type when representable and otherwise defaults to `i32`. A floating
literal contains a decimal point or exponent, accepts an `f32` or `f64` suffix,
uses an expected floating type when present, and otherwise defaults to `f64`.
Unary `-` is an operator rather than part of a literal token, but literal range
checking includes an immediately applied minus so each signed type's minimum
value is expressible.

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
is a one-element tuple. `str` is an immutable UTF-8 character sequence.
`String` is the standard-library mutable UTF-8 sequence type. A copied `String`
is an independent logical value; an implementation may use copy-on-write
storage. `str` qualifies for `StableHash`; `String` does not.

A string literal materializes as `str` or `String` when an expected type is
available from a binding annotation, field, argument, or return position. With
no expected type, its type defaults to `str`. Contextual literal materialization
does not create a general implicit conversion: an existing `str` value must use
an explicit operation such as `String.from(text)` to produce a `String`.
Replacing a `str`-typed field through a mutable aggregate path is valid, but the
contents of an existing `str` value cannot be mutated. Formatting is defined in
Section 7.

A fixed array type is `[T; N]`, where `N` is a compile-time nonnegative `usize`
value. `[first, second]` constructs an array. The compiler-handled built-in
macro forms `@vec[first, second]`, `@map{key: value, ...}`, and
`@set{value, ...}` construct a `Vec`, `Map`, and `Set`, respectively. The
`@name` namespace is reserved for macro invocation; these three built-ins are
the only macro forms in the initial language, and user-defined macros are not
yet supported. The lowercase macro names are distinct from the `Vec[T]`,
`Map[K, V]`, and `Set[T]` type names.

Literal elements and map entries are evaluated left-to-right. Their types must
produce one exact element, key, or value type after contextual literal
materialization. An empty array, vector, map, or set literal requires an
expected collection type. Multiline collection literals permit trailing
commas. A later duplicate map key replaces the earlier value, while duplicate
set elements collapse to one element.

Arrays are ordinary fixed-size aggregates and follow recursive logical value
copying. Arrays qualify for `StableHash` when their element type does.
`Vec.new()`, `Map.new()`, and `Set.new()` are the ordinary associated functions
for empty collections; populated construction uses the corresponding literal
form.

The standard-library growable sequence type is `Vec[T]`. `Vector` is not an
alternative name for this type.

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
`StableHash`.

Array and `Vec` indices have type `usize`. Indexing either in value context
produces an ordinary independent copy of the selected element. An out-of-bounds
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
`remove` returns the removed element as an ordinary independent value.

Indexing a `Map[K, V]` with a `K` in value context independently copies the
stored value and traps when the key is absent. Through a mutable map path, an
indexed value may be replaced or directly mutated as an assignable place, but
it is not addressable for reference formation. An indexed mutable place requires
an existing key and traps if the key is absent; insertion uses `insert`. Map key
arguments are passed by the language's ordinary copy semantics. `Map` provides `len() -> usize`,
`is_empty() -> bool`, `contains_key(key) -> bool`,
`get(key) -> Option[V]`, `insert(key, value) -> Option[V]`,
`remove(key) -> Option[V]`, and `clear() -> ()`. `insert` returns the replaced
value as an ordinary independent value, if any; `remove` similarly returns the
removed value.

`Set` has no indexing operation. It provides `len() -> usize`,
`is_empty() -> bool`, `contains(value) -> bool`, `insert(value) -> bool`,
`remove(value) -> bool`, and `clear() -> ()`. Its value arguments use ordinary
copy semantics. `insert` returns whether the value was newly added, and
`remove` returns whether a value was present.

### 4.2 Structs

`struct` declares an aggregate value type. Fields must appear before methods in
the struct body. A struct's inherent methods are declared in that same body;
there is no inherent `impl` block.

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

A bound call such as `value.method()` adapts only its receiver. If the method
expects `self: &Self`, an addressable value receiver is automatically borrowed
as `&value`. If it expects `self: &var Self`, an addressable mutable value is
automatically borrowed as `&var value`. A receiver that is already a suitable
reference is used directly. Receiver adaptation never upgrades `&T` to
`&var T`, and it does not apply to any non-receiver argument.

Struct literals use `Type { field: expression, ... }`. Fields may appear in any
order but every field must appear exactly once. `Type { field }` abbreviates
`Type { field: field }`. Multiline comma-separated forms permit a trailing
comma. Elamite initially has no record-update or spread expression.

Every cycle in the value-containment graph of structs and enums must cross an
explicit indirection type: `&T`,
`&var T`, `*T`, or `*var T`. Generic wrappers such as `Option[T]` and `Vec[T]`
and transparent type aliases do not break a containment cycle. This rule makes
recursive identity, aliasing, and mutability visible in source types. Hidden
managed storage used to implement a value or copy-on-write optimization does
not count as explicit indirection.

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

Structs and enums place an optional derive list in parentheses immediately after
the declaration name and any generic parameter list, as in
`struct Wrapper[T](Default, PartialEq):`. These parentheses are reserved for a
nonempty, comma-separated list of compiler-supported derivable traits. Duplicate
entries are invalid. User-defined traits may be implemented normally but cannot
define custom derive behavior. A derived implementation has no visibility
modifier separate from its type declaration.

`Default` is a built-in trait with the associated function
`fn default() -> Self`. `struct Name(Default):` derives an implementation that
supplies `Self.default()` by calling `default()` for each field. Derivation is
valid only when every field type implements `Default`. For a generic struct,
the derived implementation exists conditionally when the field types it uses
satisfy that requirement; derivation does not add bounds to the type declaration
itself. `Default` derivation is struct-only. An enum may implement `Default`
manually, but no variant is selected implicitly.

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

Safe references and function values do not implement `Default`. A struct with
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
pointers compare address identity, including comparison with `null`. References,
trait-object references, and raw pointers have no relational ordering. Function
values use their generated-function and closure-environment identity as defined
in Section 5. Content comparison through references is explicit, as in
`*left == *right`. Because recursive aggregate edges cross explicit reference
or pointer types and those edges compare by identity, compiler-derived
structural equality terminates for recursive values.

`StableHash` requires a compiler-proven stable structure together with built-in
or compiler-derived `Eq` and `Hash`. Types using manually implemented equality
or hashing do not qualify initially. `Identity[&T]` and `Identity[&var T]`
provide `Eq`, `Hash`, and `StableHash` using the referenced target's stable
managed address, allowing explicit identity-keyed maps and sets.

## 5. Functions and closures

Named function parameters require a name and type. The return type follows the
parameter list with `->`. It may be omitted for a unit-returning function. A
non-unit function must explicitly return a value with `return expression` on
every reachable path. Elamite has no implicit tail-expression return: the value
of an expression used as a statement is discarded even when it is the final
statement in a body. Falling off the end, or using `return` without an
expression, is valid only for a unit-returning function.

Elamite does not support function overloading. A declaration namespace may
contain only one function of a given name, regardless of parameter types,
return type, or generic parameters. Generic functions and distinct names are
the alternatives for type-specific behavior. This rule does not decide
collisions between inherent and trait methods, which are governed by method
resolution.

Function parameters cannot have default values. Every call to a non-variadic
function must provide exactly its declared number of arguments, so every
non-variadic `fn(Args) -> Return` value has one fixed arity.

A final parameter may use the variadic form `name: ...T`. It accepts zero or
more trailing arguments, each of type `T`, and binds `name` inside the function
to the slice type `[T]`. Variadics are homogeneous and may appear only once,
as the final parameter. A variadic function value preserves the marker in its
function type, for example `fn(i32, ...String) -> ()`. Elamite lowers this
form as a slice argument rather than as C's untyped variadic calling
convention.

~~~elx
fn apply_offset(callback: fn(i32) -> i32, value: i32) -> i32:
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

Function values use `fn(Parameters) -> Return`. Referencing a named function
or an unbound method produces a function value. Anonymous closures use `fn`
with a parameter list and an indented body. Every closure parameter requires a
name and explicit type. `fn():` is the parameterless form; `fn:` is invalid. The
closure body begins on the line after the colon. A closure may omit its return
type when it can be inferred from an expected function type or consistently
from its explicit `return` paths. Non-unit closure results still require
`return expression`; closures do not use tail-expression returns.

A function value is an ordinary storable value. It may appear in a binding,
field, enum payload, collection element, parameter, or return value. Named
functions, instantiated generic functions, unbound methods, and closures are
compatible only when parameter types, return type, arity, and any variadic
marker match exactly. Function types have no variance or implicit signature
adaptation, and collections of them are homogeneous by complete function type.

A generic function becomes a function value only after all of its type
arguments are determined explicitly or by an expected function type. Elamite
initially has no erased any-callable type, dynamically erased call-operator,
runtime signature inspection, or heterogeneous function-value collection.
Ordinary `dyn Trait` method dispatch is defined separately and does not make a
trait object directly callable with `object(args)`. Stored capturing closures
retain their shared GC-managed environment.

Selecting a method from a type produces its unbound function value. Selecting a
method from an instance does not produce a function value; an instance method
may be called directly, but a bare expression such as `session.stop` is
invalid. A callback bound to a receiver must use an explicit closure. An
unbound method retains its declared receiver parameter, so its caller must form
any required reference explicitly. A trait-qualified method selection is also
unbound and follows the same rule.

~~~elx
let stop: fn(&var Session) -> () = Session.stop // unbound method
// let callback = session.stop                  // invalid
let callback = fn():
    session.stop()                // closure captures `session`
~~~

A closure automatically copies each free local binding that it uses when the
`fn` expression is evaluated. An ordinary captured value is an independent
logical value: later mutation or assignment through the outer binding does not
change the closure's copy. Capturing an `&T` or `&var T` value copies that
reference, so the closure continues to name the same reference target. The
closure environment is managed by the garbage collector and may outlive its
creating scope.

A captured `let` binding is non-rebindable and is not a mutable place. A
captured `var` binding is a closure-owned mutable copy: mutation changes that
copy, persists for later calls to the same closure, and never changes the outer
binding. A captured `&var T` retains its target-mutation capability regardless
of whether the binding containing that reference was declared with `let`.

Assigning, passing, or returning a capturing closure copies its identity-bearing
callable handle. All handle copies share the one managed closure environment and
therefore share its captured mutable state. A named function or an unbound
method has no captured environment.

Function values support `==` and `!=` as callable-identity comparisons. Two
function values are equal only when they name the same generated function and
the same closure environment. Named functions and unbound methods have no
environment. Function equality does not compare captured values or determine
whether two functions have equivalent behavior.

Named functions may call themselves and other named functions declared in the
same lexical scope. A contiguous group of local `let` bindings whose
initializers are `fn` expressions forms a recursive closure group. Every name
in that group is visible in every group closure body, including its own body.
The runtime creates stable managed callable cells for the group and initializes
them before any closure in the group may be invoked. This permits direct and
mutual recursive closures without a separate `rec` keyword.

~~~elx
let add_one: fn(i32) -> i32 = fn(value: i32) -> i32:
    return value + 1

var offset = 1
let add_offset = fn(value: i32) -> i32:
    return value + offset

offset = 3
println(add_offset(41)) // 42: `offset` was captured as 1

var count = 0
let bump = fn() -> i32:
    count += 1
    return count

println(bump()) // 1
println(bump()) // 2
println(count)  // 0

var shared_count = 0
let first = fn() -> i32:
    shared_count += 1
    return shared_count
let second = first

println(first())  // 1
println(second()) // 2
println(first())  // 3: both handles share one environment

type Counter = fn() -> i32

fn make_counter() -> Counter:
    var counter = 0
    return fn() -> i32:
        counter += 1
        return counter

let independent = make_counter()
println(first == second)       // true: same closure environment
println(first == independent)  // false: different environment

let countdown = fn(value: i32) -> i32:
    if value == 0:
        return 0
    return countdown(value - 1)

let is_even = fn(value: i32) -> bool:
    if value == 0:
        return true
    return is_odd(value - 1)

let is_odd = fn(value: i32) -> bool:
    if value == 0:
        return false
    return is_even(value - 1)

println(countdown(3)) // 0
println(is_even(4))   // true

var session = Session { active: true, name: "build" }
var stop_callback = fn():
    session.stop()
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
Explicit trait objects provide dynamic dispatch. `dyn Trait` denotes a trait
object and initially appears only behind a safe reference as `&dyn Trait` or
`&var dyn Trait`; bare trait-object values and raw pointers to trait objects are
invalid. A concrete reference may safely coerce to a trait-object reference of
matching mutability when its target type implements the trait. The object is a
fat reference containing the managed target reference and a static vtable.

A trait is object-safe when every method available through the object has an
`&Self` or `&var Self` receiver, has no method-level generic parameters, and
does not otherwise mention `Self` in its parameter or return types. A trait that
fails these rules remains usable with static dispatch but cannot form
`dyn Trait`. A generic trait can form an object only after all of its trait type
arguments are concrete. Default methods participate in the vtable.

Trait-object calls dispatch through the vtable, and different concrete target
types may coexist in a homogeneous collection such as `Vec[&dyn Trait]`.
Trait objects initially provide no downcasting, runtime concrete-type
inspection, or multi-trait object composition. Safe-reference reachability and
escape promotion apply to their concrete targets.

A `pub trait` exposes all of its methods wherever the trait is accessible.
Trait method declarations and implementation methods cannot carry separate
`pub` modifiers.

Bound method lookup considers inherent methods and methods from traits in the
current lexical scope. An inherent method wins over a same-named trait method.
If multiple in-scope traits otherwise provide a matching method, the call is
ambiguous. Explicit `Type.Trait.method(receiver, ...)` qualification selects one
trait method and bypasses bound-call ambiguity. Trait-qualified methods are
unbound and retain their declared receiver type, so callers form any required
reference explicitly.

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

let dynamic_toggle: &dyn Toggle = &session
println(dynamic_toggle.status()) // dynamically dispatched
~~~

## 7. Expressions and control flow

`if`, `else`, `match`, `for`, and `while` use indentation-delimited bodies.
Conditions appear after the keyword. `match` evaluates its scrutinee and chooses
the first matching arm. Each arm uses `Pattern:` followed on the next line by
an indented body.

Patterns initially appear only in `match` arms. They include `_`, immutable
binding names, primitive and `str` literals, tuples, structs, unit/tuple/record
enum variants, and alternatives separated by `|`. Struct and record-variant
patterns use named fields. Field shorthand such as `Point { x, .. }` binds `x`
and ignores the remaining fields; without `..`, every field must appear.
Alternative patterns must bind the same names with the same types.

A guarded arm uses `Pattern if condition:`. Its bindings are in scope in the
boolean guard. A failed guard proceeds to the next arm, and guarded arms do not
contribute to exhaustiveness. Pattern bindings receive ordinary independent
value copies and behave as `let` bindings. Matching a reference does not
implicitly dereference it; code matches `*reference` when content matching is
intended.

Every match is exhaustive. Patterns over an infinite domain require a catch-all
binding or `_`. Arms are tested in source order and never fall through. A
statically unreachable arm is a compile-time error.

Control-flow constructs, `with` and `unsafe` blocks, and indented bodies are
statements, not value-producing expressions. They complete with unit but cannot
appear in a context that requires a value. Assignment and compound assignment
are also statements and cannot be nested inside expressions. Elamite has no
`++` or `--` operators. Compound assignment evaluates its destination place
exactly once.

Expression evaluation is left-to-right. A call evaluates its callee or receiver
first and then each argument in source order. `&&` and `||` require `bool` and
short-circuit; `!` is boolean negation. Unary `+` accepts numeric values, unary
`-` accepts signed integers and floating-point values, and `~` accepts integers.
Arithmetic and bitwise operators are initially built-in rather than
user-overloadable; comparison operators use the traits defined in Section 4.5.
`%` accepts integers only. A shift count must have an unsigned integer type and
be smaller than the bit width of the left operand. Chained comparisons such as
`a < b < c` are invalid.

Operator precedence from highest to lowest is:

1. Field access, calls, indexing, and postfix `?`.
2. Unary `!`, `~`, `+`, `-`, dereference, `&`, and `&var`.
3. `as`.
4. `*`, `/`, and `%`.
5. `+` and `-`.
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

The initial `for` statement directly supports arrays, `Vec`, `Map`, and `Set`;
there is no user-defined iteration protocol or source-level iterator type yet.
The iterable expression is evaluated exactly once and copied into hidden loop
state using ordinary logical value semantics. Later mutation of the source
collection therefore cannot affect the active loop. An implementation may use
copy-on-write storage so long as this independence remains unobservable.

Arrays and vectors iterate in index order. Maps yield `(K, V)` pairs, and sets
yield their elements; map and set iteration order is unspecified and may vary
between executions. Each yielded element, key, or value is independently copied
into the loop's non-rebindable binding. Iteration exposes no safe references to
collection interiors. It visits only direct elements and does not recursively
traverse targets reached through explicit reference-like values.

### 7.2 Formatted strings and display

`Display` is a compiler-recognized prelude trait with a formatting method that
writes a value to a mutable standard-library `Formatter`. Users may implement
it normally. Primitive values, `str`, `String`, references to displayable
values, and standard collections of displayable values provide implementations.

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
copies `value` and makes the independent logical value the value of the postfix
expression. `Err(error)` independently copies `error` and immediately returns
`Result.Err(error)` from the enclosing function.

`?` is the explicit exception to the general requirement that returning from a
function uses `return`. It performs no implicit error conversion. A caller must
convert a different error type explicitly, such as with `match`, before
applying `?`. `Option[T]` is handled with `match` rather than `?`.

~~~elx
fn increment_result[E](result: Result[i32, E]) -> Result[i32, E]:
    let value = result?
    return Result.Ok(value + 1)
~~~

Elamite has no `defer` statement and no implicit destruction protocol. Garbage
collection manages memory only. Deterministic external-resource cleanup uses
the compiler-known prelude trait `Close` and a lexical `with` statement.

`Close` is an ordinary implementable trait with this method:

~~~elx
trait Close:
    fn close(self: &Self) -> ()
~~~

`close()` may be called explicitly. It must be idempotent: calling it on an
already closed resource has no effect. It returns unit and must handle any
recoverable cleanup errors internally. A resource that needs fallible flushing,
committing, or finalization provides a separate explicit operation returning
`Result`; that operation is not part of `Close`.

A type implementing `Close` represents an identity-bearing shared resource
handle and is an explicit exception to independent ordinary-value copying.
Copying the handle must retain one shared managed resource state. Closing
through any copy closes that resource for every handle. Operations that require
an open standard resource return an appropriate error after it is closed. Manual
`Close` implementations are responsible for obeying these laws, just as manual
comparison implementations are responsible for their trait laws.

`with expression as name:` evaluates `expression` exactly once and stores its
value in a hidden binding whose type must implement `Close`. The optional
`as name` clause gives that non-rebindable binding a name within the body; when
omitted, only the compiler can access the hidden binding. There is no separate
entry hook. An error propagated while evaluating the expression prevents entry
into the body and creates no cleanup obligation.

The compiler calls `close()` when control exits a `with` body by falling
through, explicit `return`, or `?` propagation. Nested `with` bodies therefore
close from innermost to outermost. Calling `close()` explicitly inside the body
is valid because the automatic call is idempotent. An unrecoverable trap,
including a trap during `close`, and out-of-memory termination do not guarantee
further cleanup.

~~~elx
with File.open("report.txt", "w")? as file:
    file.write("Elamite report")?
~~~

Ordinary scope exit and garbage collection never call `close()`. A resource
that is neither explicitly closed nor placed in a `with` body may therefore
leak its external resource. An implementation may warn about leaks it can
prove locally, but such diagnostics are not required to be complete.

## 9. Garbage collection

Elamite uses the non-moving Boehm garbage collector for managed memory. Stack
versus managed-heap placement is unobservable in safe code. Escape promotion
preserves safe-reference behavior and `Identity` identity. Once created, a
managed allocation does not move during its lifetime.

Managed storage remains alive while it is reachable through a strong language
path. Strong roots include every local binding until its lexical scope ends,
function parameters for the complete call, temporaries until their full
expression finishes, module-level values, safe references, and managed handles
stored inside structs, enums, collections, closures, and hidden loop state.
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
collection callbacks. Garbage collection never invokes `Close`. The runtime
may perform internal reclamation work only when it invokes no user code and
creates no observable external-resource cleanup behavior.

Managed allocation failure is unrecoverable because ordinary copying, closure
creation, copy-on-write mutation, and escape promotion may allocate implicitly.
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

Unsafe functions are declared with `unsafe`. An `unsafe:` block permits
operations that require the unsafe boundary, including raw-pointer conversion to
references.

~~~elx
unsafe pub fn from_pointer(pointer: *Session) -> &Session:
    unsafe:
        return pointer as &Session
~~~

The compiler should diagnose provably invalid unsafe reference construction and
may warn when a raw pointer's validity cannot be established. This diagnostic
does not apply merely because a safely formed reference to a local binding
escapes; such storage is promoted. Exact diagnostics, FFI marshalling,
promotion and root registration, callback retention, pointer ownership, and
foreign exceptions are open in
[I-016](ISSUES.md#i-016-foreign-function-interface-and-unsafe-code) and
[I-020](ISSUES.md#i-020-raw-pointer-provenance-and-violations).

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
