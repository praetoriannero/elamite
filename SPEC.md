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
data types, deterministic cleanup, raw pointers behind an unsafe boundary, and
indentation-delimited control flow.

Ordinary values are passed and assigned by copy. Passing `&value` explicitly
passes a shared reference; passing `&var value` explicitly passes a mutable
reference. Elamite has no source lifetime parameters. Managed memory uses Boehm
GC; programs should not use collection timing for resource cleanup.

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

Declarations and control-flow blocks use a trailing `:` followed by an
indented body. The body ends at the next dedent. This form is used for `mod`,
`struct`, `enum`, `trait`, `impl`, `if`, `else`, `match`, `for`, `while`,
anonymous closures, and most function declarations.

Record literals use braces. Parentheses form tuples and group expressions.
Statements normally end at a newline. Exact continuation, tab, empty-body,
and brace-body rules remain open in [I-018](ISSUES.md#i-018-mixed-body-and-return-syntax).

~~~elx
if enabled:
    println("enabled")
else:
    println("disabled")

let point = Point { x: 1.0, y: 2.0 }
let pair = (point.x, point.y)
~~~

### 2.3 Modules, imports, and visibility

A source file is a module root. `mod` introduces a nested module. `import` is
permitted at module level, including within a nested module, and brings the
final component of its path into that module's scope. Paths use periods.

Top-level declarations are private unless prefixed with `pub`. `pub` applies to
modules, functions, structs, enums, traits, and type aliases. Fields and
methods are private unless individually marked `pub`.

~~~elx
import std.io

pub mod diagnostics:
    import std.io

    pub fn report(message: String):
        io.println(message)

pub type UserId = u64
~~~

Package roots, re-exports, and import conflict rules remain open in
[I-011](ISSUES.md#i-011-modules-packages-and-visibility).

## 3. Values, mutability, and references

### 3.1 Copying values

`let` creates an immutable binding. `var` creates a mutable, rebindable
binding. Assignment, ordinary argument passing, and ordinary returns copy the
source value; using the source after that operation is valid. This shallow-copy
capability is implicit for ordinary values and is not an opt-in trait.

A shallow copy creates independent outer storage. Primitive fields and `str`
fields copy directly. Fields whose values are `String`, `struct`, enum, or
collection values preserve their reference to the same managed subvalue. Thus,
replacing a direct field changes only the copied outer value, while mutating a
nested aggregate is observed through every shallow copy that shares it.

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
println(original.address.city) // "Beacon"
~~~

### 3.2 References

`&T` is a shared reference to `T`. `&var T` is a mutable reference to `T`.
The expression `&value` forms a shared reference, and `&var value` forms a
mutable reference to a mutable place. Reference field and method access
automatically dereferences the reference.

Reference formation is always explicit. A context that expects `&T` or
`&var T` never implicitly converts a `T` place; the source expression must use
`&value` or `&var value`.

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
`&Point { x: 0.0, y: 0.0 }` creates an anonymous, GC-managed value and
references it.

~~~elx
let point: &Point = &Point { x: 0.0, y: 0.0 } // valid
let from_call: &Point = &make_point()         // invalid
let from_sum: &i32 = &(left + right)          // invalid
~~~

Interior values of `Vec`, `Map`, and `Set` are addressable. A reference to an
interior value initially refers to the collection's current backing-storage
snapshot. When the collection is mutated, it receives new backing storage and
any older backing-storage snapshot that is still referenced is retained by the
garbage collector. The existing reference therefore becomes a stable detached
reference: it continues to observe its old value rather than a later value at
the same index, key, or set position.

This snapshot is shallow. If the referenced value contains managed subvalues,
the old and new collection snapshots still share those subvalues according to
the ordinary copy rules. A mutable reference updates its current target; after
the collection detaches, that target is the retained snapshot rather than the
collection's replacement storage.

~~~elx
var points = Vec.new(Point { x: 0.0, y: 0.0 })
let first: &var Point = &var points[0]

points[0] = Point { x: 1.0, y: 1.0 } // detaches the old backing storage
first.x = 5.0

println(points[0].x) // 1.0
println(first.x)     // 5.0
~~~

References are valid struct fields, enum payloads, collection elements, closure
captures, parameter types, and return types. A reference held in any of those
locations keeps its target reachable through the garbage collector.

A reference formed directly from a binding points to that binding's storage.
It observes later assignments to the binding, following the ordinary C and Go
pointer model.

~~~elx
var point = Point { x: 0.0, y: 0.0 }
let view: &Point = &point

point = Point { x: 1.0, y: 1.0 }
println(view.x) // 1.0
~~~

A reference path that enters a nested managed aggregate targets that selected
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

Raw pointer types are `*T` and `*var T`. A raw pointer can be `null`, unlike an
ordinary reference. `&T` may convert safely to `*T`; `&var T` may convert safely
to `*var T`. Converting a raw pointer to a reference requires an `unsafe`
context.

~~~elx
var pointer: *i32 = null

unsafe:
    let reference: &i32 = pointer as &i32
~~~

The validity of dereferencing a null pointer, pointer truthiness, pointer casts,
and the compiler's treatment of unsafe references are open in
[I-020](ISSUES.md#i-020-null-pointers-and-unsafe-references).

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

Tuples use parentheses, for example `(bool, String)`. `str` is an immutable
UTF-8 character sequence. `String` is the standard-library mutable UTF-8
vector type. The ownership, copying, literal, conversion, and formatting rules
for strings remain open in [I-013](ISSUES.md#i-013-core-types-literals-and-expressions).

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

`Self` denotes the enclosing struct type. A plain `self: Self` parameter
receives a copied receiver. `self: &Self` and `self: &var Self` receive shared
and mutable references respectively.

Struct literals name their fields in braces. A struct may recursively contain
itself only when every recursive path crosses an explicit reference type. A
direct recursive value field is invalid. This keeps recursive indirection and
mutability explicit without a source-level `Box`.

~~~elx
struct Chain[T]:
    value: T
    next: Option[&Chain[T]]

struct MutableChain[T]:
    value: T
    next: Option[&var MutableChain[T]]

let leaf = Chain { value: 1, next: Option.None }
let root = Chain { value: 2, next: Option.Some(&leaf) }

// Invalid: `Node` has no finite direct value representation.
// struct Node:
//     next: Option[Node]
~~~

### 4.3 `Default` derivation and initializers

`struct Name(Default):` derives the `Default` trait. A derived `Default`
implementation supplies `Self.default()`. A `new` method may call that method
to construct an instance.

~~~elx
struct Point(Default):
    x: f64
    y: f64

    pub fn new() -> Self:
        return Self.default()
~~~

A struct containing an `&T` field cannot derive `Default`, because a default
reference target is unknown. `Option[T]` is the recommended representation for
an optional value. Raw pointers may default to `null`.

The exact derive grammar, default values, interaction with generic fields, and
status of `new` as a convention rather than a keyword are open in
[I-021](ISSUES.md#i-021-default-initialization-and-optionals).

### 4.4 `Clone`

`Clone` is an opt-in trait for explicit duplication beyond the language's
ordinary shallow copy. It declares one method:

~~~elx
trait Clone:
    fn clone(self: &Self) -> Self
~~~

`struct Type(Clone):` derives a structural clone implementation. A derived
implementation calls `clone()` for each ordinary field and copies explicit
references and raw pointers unchanged. It is valid only when every ordinary
field implements `Clone`. A type may instead implement `Clone` manually; that
implementation defines its own behavior for recursive values, cycles, sharing,
and external state.

Consequently, `clone()` may create a fully independent aggregate graph when
each participating field's implementation does so. Explicit `&T` and `&var T`
references always remain aliases to their original targets when copied by a
derived clone.

`Clone` is not implicit. A type without a derived or manual implementation does
not have a `clone()` method. There is no `!Copy` marker: ordinary shallow copying
is a core property of every Elamite value.

### 4.5 Enums, optionals, and aliases

Enums are tagged unions with unit-like, tuple-like, or record-like variants.
`Option[T]` represents a possibly absent value. Elamite has no trailing
optional-type syntax. Like structs, an enum's recursive paths must cross an
explicit reference type.

~~~elx
enum Result[T, E]:
    Ok(T)
    Err(E)

enum Option[T]:
    Some(T)
    None

enum State:
    Count(i32)
    Disabled
~~~

A module-level `type` alias is transparent. Generic type parameters and
arguments use square brackets.

~~~elx
type NameMap[V] = Map[String, V]
~~~

## 5. Functions and closures

Named function parameters require a name and type. The return type follows the
parameter list with `->`. It may be omitted for a unit-returning function. A
function body may use explicit `return` or end in a result expression.

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
    callback(value)

fn session_status(session: &Session) -> str:
    if session.active:
        return "active"
    else:
        return "inactive"

pub fn variadic(x: i32, y: ...String) -> ():
    for value in y:
        println("{}: {}", x, value)

variadic(7)
variadic(7, "one", "two")
~~~

Function values use `fn(Parameters) -> Return`. Referencing a named function
or an unbound method produces a function value. Anonymous closures use `fn`
with a parameter list and body; `fn:` is the parameterless closure form shown
by the current demonstration.

Selecting a method from a type produces its unbound function value. Selecting a
method from an instance does not produce a function value; an instance method
may be called directly, but a bare expression such as `session.stop` is
invalid. A callback bound to a receiver must use an explicit closure.

~~~elx
let stop: fn(&var Session) -> () = Session.stop // unbound method
// let callback = session.stop                  // invalid
let callback = fn: session.stop()                // closure captures `session`
~~~

A closure automatically shallow-captures each free local binding that it uses
when the `fn` expression is evaluated. An ordinary captured value is therefore
a snapshot: later assignment to the outer binding does not change the closure's
copy. Capturing an `&T` or `&var T` value copies that reference, so the closure
continues to name the same reference target. The closure environment is managed
by the garbage collector and may outlive its creating scope.

A captured `let` binding is immutable. A captured `var` binding is a
closure-owned mutable copy: mutation changes that copy, persists for later
calls to the same closure, and never changes the outer binding.

Assigning, passing, or returning a capturing closure shallow-copies its
callable handle. All copies share the one managed closure environment and
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
var stop_callback = fn: session.stop()
~~~

The exact anonymous closure grammar remains open in
[I-005](ISSUES.md#i-005-function-method-and-closure-values).

## 6. Generics and traits

Generic declarations use square brackets. A trait declares behavior and is
implemented with `impl Trait for Type`. Traits may declare required methods.
The current syntax uses ordinary methods inside the trait implementation.

~~~elx
trait Toggle:
    fn status(self: &Self) -> String

impl Toggle for Session:
    fn status(self: &Self) -> String:
        if self.active:
            return String("trait active")
        else:
            return String("trait inactive")
~~~

Trait visibility, imports, default methods, collisions with struct methods,
coherence, overlap, dispatch, and trait-object support remain open in
[I-006](ISSUES.md#i-006-traits-and-method-resolution).

## 7. Expressions and control flow

`if`, `else`, `match`, `for`, and `while` use indentation-delimited bodies.
Conditions appear after the keyword. `match` evaluates its scrutinee and chooses
the first matching arm. The current arm form uses `Pattern:` followed by its
body.

~~~elx
fn value_or[T](node: Option[T], fallback: T) -> T:
    match node:
        Option.Some(value): return value
        Option.None:
            return fallback

var retries = 0
while retries < 2:
    println("retry {}", retries)
    retries = retries + 1

for value in Vec.new(1, 2, 3):
    println("value {}", value)
~~~

Pattern grammar, match exhaustiveness, value-versus-statement block rules,
operators, casts, collection literals, and numeric conversion are open in
[I-013](ISSUES.md#i-013-core-types-literals-and-expressions).

## 8. Errors and deterministic cleanup

Recoverable errors use `Result[T, E]`. Postfix `?` propagates an error from a
`Result` expression to the enclosing `Result`-returning function. `Option[T]`
is handled with `match` rather than `?`.

`defer call(args)` schedules its call for scope exit. It is intended for
deterministic external-resource cleanup and is distinct from garbage
collection.

~~~elx
fn write_report(path: String, contents: String) -> Result[(), IoError]:
    let file: File = File.open(path, "w")?
    defer file.close()

    file.write(contents)?
    return Result.Ok(())
~~~

Error conversion, defer ordering, failures during cleanup, and resource-copy
semantics remain open in [I-010](ISSUES.md#i-010-errors-cleanup-and-resource-values).

## 9. Garbage collection

Elamite uses Boehm GC for managed memory. Managed allocations may be reclaimed
after they become unreachable. The collector does not close files, release
foreign handles, or otherwise provide deterministic external-resource cleanup.

The precise root, weak-reference, finalization, allocation-failure, and
runtime-debugging contract remains open in
[I-014](ISSUES.md#i-014-garbage-collector-and-runtime-contract).

## 10. Unsafe operations and C interoperability

Unsafe functions are declared with `unsafe`. An `unsafe:` block permits
operations that require the unsafe boundary, including raw-pointer conversion to
references.

~~~elx
unsafe pub fn from_pointer(pointer: *Session) -> &Session:
    unsafe:
        return pointer as &Session
~~~

The compiler should diagnose suspicious unsafe reference construction, such as
returning a reference whose validity cannot be established. Exact diagnostics,
FFI marshalling, pinning, callback retention, pointer ownership, and foreign
exceptions are open in [I-016](ISSUES.md#i-016-foreign-function-interface-and-unsafe-code)
and [I-020](ISSUES.md#i-020-null-pointers-and-unsafe-references).

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
