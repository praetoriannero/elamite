# C ABI and FFI demo

This package exercises the implemented C boundary in both directions:

- every ABI-safe scalar type;
- a C struct passed and returned by value;
- an opaque C type used through raw pointers;
- a C `void` return;
- an Elamite function passed to C as a callback; and
- C calling an Elamite function with a stable `@exportc` symbol.

From the repository root, run:

```sh
cargo run -- run examples/c_ffi
```

The program prints each result beside its expected value. Imported C calls and
raw-pointer operations are grouped in `unsafe:` because Elamite treats every
foreign call as unsafe.
