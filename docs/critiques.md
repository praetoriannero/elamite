# Elamite Design Critiques

> This document records outside critical review of the language design. It is
> opinion, not specification: nothing here is normative, and nothing here
> obligates an implementation. Unresolved design *questions* belong in
> `issues.md`; settled decisions belong in `spec.md`; implementation rationale
> belongs in `ledger.md`. A critique that becomes an accepted design change
> should move to `issues.md` and then out of this file.
>
> Review basis: `spec.md`, `roadmap.md`, `ledger.md`, and the adversarial
> regression packages under `tests/fixtures/regression`. The reviewer has read
> the specification closely but has not written substantial programs in the
> language, so these notes address design coherence rather than ergonomics in
> practice.

## 1. What the design gets right

Recorded first because the critiques below are narrow, and a reader who sees
only the critiques will misjudge the balance.

**The central thesis is coherent and uncommon.** Deep value semantics, with
aliasing confined to explicitly-typed carriers, and garbage collection handling
lifetime, is a real point in the design space rather than a compromise between
existing ones. `spec.md` 3.1 commits to it without hedging: "Copying is a core
property of every value and is not controlled by a trait." The sharp edge —
that explicit aliases survive a copy while ordinary fields do not — is
specified precisely where most languages become vague.

**Specification discipline exceeds what most languages have at 1.0.**
`ledger.md` maps every normative rule to an implementation milestone, runtime
dependency, and test layer. The adversarial audit found fifteen runtime
defects and four compile-time ones and closed all of them. Claims about the
implementation are backed by evidence rather than assertion.

**`spec.md` 10.4 is the strongest chapter.** `Transfer` as a compiler-recognized
structural capability rather than a user-implemented marker; a `Mutex[T]` that
exposes no `&T`, no `&var T`, and no guard, making reference escape from
protected storage categorically impossible; process-fatal traps that render
poisoning unnecessary; sequential consistency with no relaxed-ordering surface.
Each deletes a class of defect instead of documenting it. The no-guard mutex in
particular is a decision most languages get wrong and can never reverse.

## 2. Principal critique: copy costs have no normative guarantee

> Current response: partially addressed. `cost_model.md` now publishes the
> measured eager-copy implementation model and its maintenance contract, while
> `roadmap.md` keeps **Value-copy and allocation optimization** as the next
> milestone. The remaining critique is about implementation cost and normative
> asymptotic guarantees, not an absence of implementation documentation.

### 2.1 The gap

`spec.md` 3.1 permits copy-on-write storage:

> An implementation may share immutable or copy-on-write backing storage, but
> such sharing cannot be observed through language operations.

This is a license to optimize. It is not a guarantee a programmer can build on.
Three facts together make that the language's most significant open risk:

1. **The specification contains no normative cost model.** `cost_model.md`
   documents current costs and measurements, but `spec.md` deliberately makes
   no asymptotic promise. Every semantic property is pinned down — a reader can
   predict exactly which `BuiltinTrap` identity a bad index raises — while the
   future cost of the language's most common operation remains unguaranteed.

2. **Copy-on-write is not implemented yet.** `roadmap.md` now commits planned
   value-copy and allocation work, including COW text and collections, but the
   shipped representation remains eager. `roadmap.md` 1 still correctly states
   that "deep copying and conservative heap promotion are acceptable first
   implementations."

3. **Copying is the default calling convention.** Assignment, ordinary argument
   passing, and ordinary returns all copy (`spec.md` 3.1). The fast path — `&T`
   — is opt-in.

The honest current statement of the implementation cost is therefore: *every
copy of a `String`, `Vec`, `Map`, or `Set` is O(n).* The language specification
still permits a conforming implementation to keep it that way even though the
compiler roadmap now plans otherwise. For a language whose defining
characteristic is that copies are everywhere, this remains the load-bearing
gap until the optimized model lands or reviewed normative bounds are accepted.

### 2.2 Why "unobservable" is the wrong framing

The clause "such sharing cannot be observed through language operations" treats
physical sharing as an implementation detail whose visibility is a hazard to be
suppressed. That framing is correct for *semantics* and wrong for *cost*.

Whether `f(large_table)` is O(1) or O(n) changes no value the program can
print. It entirely determines whether the program is usable. In a language
where the naive program is copy-heavy by construction, cost is the observable
that matters, and it is the one property left to chance.

The consequence is an inverted performance ergonomic: idiomatic code is slow
code, and optimization means retrofitting `&T` through call graphs. This is the
reverse of Rust, where the default is cheap and `.clone()` is visible at the
call site.

### 2.3 The hard obstacle is already designed away

An objection to normative copy-on-write in this language would be interior
references: if `&var v[0]` can be outstanding when a sibling copy forces a
detach, the reference is left naming stale storage. Rust avoids this only
because borrow checking guarantees no outstanding references at
`Rc::make_mut`, and Elamite performs no borrow or alias checking (`spec.md`
3.2).

Elamite closes the hole a different way. `spec.md` 3.2:

> Collection interiors are never addressable for safe-reference formation.
> Neither shared nor mutable references may be formed to array or `Vec`
> elements, `Map` keys or values, or `Set` elements.

References name *places* — a binding or a field — not backing storage. A detach
rewrites the backing pointer held in that place, and every reference to the
place follows it. `&var value.items` references the field, not the buffer.

This decision is what makes normative copy-on-write tractable, and it has
already been made. The expensive design work is done; the guarantee is simply
not written down.

### 2.4 Proposed direction

A normative section — for example `spec.md` 3.4, "Cost of copying" — stating
obligations rather than permissions:

- Copying a `String`, `Vec[T]`, `Map[K, V]`, or `Set[T]` shall be O(1) in time
  and space, independent of length.
- Copying a struct, tuple, or fixed array shall be O(k) in its number of direct
  fields or elements, with each field's own guarantee applying recursively. A
  copy-on-write-backed field contributes O(1).
- The first mutation through a value whose backing storage is shared may be
  O(n) — the detach. Subsequent mutations cost the ordinary cost of the
  operation.
- Consequently: passing a value to a function, returning it, or binding it
  shall be O(1) in the size of its heap-allocated contents.

The final clause is what makes idiomatic Elamite viable, and it is precisely
the promise the language does not currently make.

The amortization should be stated directly as well: N read-only copies of one
value cost O(1) each, so a read-only workload never pays for copies it does not
mutate. This is the property that decides whether spawning N threads over one
shared table is O(n) or O(N·n), and `spec.md` 10.4 currently leaves it to
implementation choice.

### 2.5 What this commits an implementation to

Stated honestly, because the cost is real:

- **A representation change.** Every copy-on-write type carries a reference
  count. This should be checked against the C ABI type rules in `spec.md` 10.1
  before being committed to.
- **A detach check on every mutation.** `values[index] = x` and `text.push(c)`
  become a predictable branch, permanently, in the hot path.
- **Atomic reference-count traffic once threads exist.** `spec.md` 10.4 already
  requires that copy-on-write reference counts, reads, and detach-on-write
  operations be thread-safe for storage to remain shared across a transfer.
  Naively this is an atomic read-modify-write on every copy and destruction,
  including in single-threaded programs. Biased or thread-local reference
  counting recovers most of the cost but is additional work.

One property materially reduces the difficulty. **Because the collector owns
reclamation, the reference count drives only a performance decision, never a
safety one.** It answers "am I shared, must I detach?" and not "may I free?"
Erring toward *detach* is always semantically correct and merely slower, so the
count may be conservative and approximate in the safe direction. This is a
substantially weaker obligation than `Rc::make_mut`, where an undercount is
memory corruption. It also permits shipping a correct implementation early and
tightening precision later without touching semantics.

### 2.6 A staged alternative

Committing to asymptotic bounds before copy-on-write exists carries its own
risk: a bound could be pinned that some future target cannot meet. A lower-risk
sequence:

1. **Completed:** publish a non-normative cost document stating current costs,
   intended improvements, instrumentation limits, and reproducible baselines.
2. Promote each bound to normative only as its `roadmap.md` package lands, one
   collection at a time, gated by the before-and-after measurement rule that
   section already requires.
3. Independently, strengthen the `spec.md` 10.4 sentence "Copy-on-write storage
   may remain physically shared" into a requirement for the committed types.
   The asymptotic difference is largest there and most surprising to users.

What should be resisted is carrying an unspecified cost model through 1.0. The
asymmetry — trap identities specified exactly, copy cost unspecified entirely —
will shape what people believe the language is for.

## 3. Secondary concerns

### 3.1 Process-fatal traps admit no in-process recovery

`spec.md` 8.1 makes runtime traps terminate the process, and `spec.md` 10.4
relies on this to justify omitting mutex poisoning. The design is internally
consistent and buys real simplification.

The cost is that an index-out-of-bounds terminates the process with no recovery
path — stricter than Go, which provides `recover`. For server-shaped workloads,
where one malformed request should not end the process, this is a significant
constraint. It is also difficult to loosen later: adding recovery after the
fact would invalidate the reasoning that makes poisoning unnecessary, and would
interact with the cleanup guarantees in `spec.md` 8.

Worth an explicit decision recorded as such, rather than an implication of the
trap design.

### 3.2 The compile-time surface may be over-built for the language's age

`spec.md` 12 specifies twelve quote roles, three separate namespaces, a bounded
interpreter with fuel accounting, hygiene and provenance tracking, and a
fixed-point expansion scheduler. It is well executed — the adversarial audit of
this surface found only four issues, all resolved.

The concern is proportion rather than quality. This is arguably more
specification surface than the core value semantics it sits above, and it is
permanent maintenance burden and permanent teaching burden. Go went fifteen
years without macros and the absence was not what constrained it.

The question worth answering explicitly: was this demanded by programs that
could not otherwise be written, or built because it was tractable?

### 3.3 Heap promotion is a second unpredictable cost

Garbage collection makes an escaping `&local` safe, and `spec.md` 9 notes that
escape promotion may allocate implicitly. `roadmap.md` lists precise escape
analysis as candidate work with conservative promotion as the fallback.

This means locals are silently heap-promoted under rules the programmer cannot
see, in a language that otherwise works hard to make costs explicit.
`cost_model.md` now documents the conservative address-taken rule; precise
escape analysis remains planned optimization work.

## 4. Summary

The design is serious and unusually disciplined, with one genuinely novel
organizing idea — deep copies, explicitly-typed aliasing, and collection for
lifetime, as an alternative to borrow checking. The concurrency chapter
indicates the design instincts are sound and improving.

The principal risk is not any individual rule. The current eager performance
model is now measured and documented, but it remains expensive and
non-normative. The value-copy optimization milestone must demonstrate that
idiomatic independent-value code can approach descriptor-copy costs before the
project considers promising asymptotic bounds in the specification.
