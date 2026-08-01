# Compile-time syntax generation

This example exercises all three stable compile-time declaration forms:

- `macro pair` expands an expression at `@pair(...)`;
- `attr identifiable` immutably adds a field and method to `Entity`; and
- `derive FieldCount` emits an ordinary trait implementation after attributes
  have finished, so it observes the added `id` field.

Run it with `elamc run examples/macros`. Use
`elamc dump expanded examples/macros` to inspect rewritten syntax, execution
order, resource use, provenance totals, and stable artifact identities.
