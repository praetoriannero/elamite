# Compiler-shipped `std` package

The `.elx` files in `src/` are compiled into the Elamite compiler and pass
through the ordinary lexer, parser, resolver, type checker, and lowering
pipeline. `src/standard.rs` contains the reviewed inventory of entities that
remain intrinsic because Elamite source cannot yet express their runtime
representation or lowering hook.

Keep the manifest and source tree valid as an ordinary `lib` package. Moving an
entity out of the intrinsic catalog requires source declarations plus
behavioral and diagnostic compatibility tests.
