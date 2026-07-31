# raylib demo

This example opens an 800×450 raylib window and lets you move a circle with
the arrow keys. The frame loop, movement, bounds checking, and drawing
coordination are written in Elamite.

The example targets desktop Linux and requires raylib development headers and
libraries that match the selected Elamite target architecture. Confirm that
the system installation is visible with:

```sh
pkg-config --cflags --libs raylib
```

Then run the example from the repository root:

```sh
cargo run -- run examples/raylib
```

Press Escape or close the window to exit.

## FFI adapter

Scalar and unit-returning raylib functions are imported directly. The
header-only `native/include/elamite_raylib.h` adapter handles window-title
text, C `bool` results, and raylib's `Vector2`/`Color` aggregate construction.
This keeps the boundary explicit while the movement and frame logic remain in
Elamite.

The manifest includes the common desktop Linux libraries needed when raylib is
linked statically. A raylib build using a different platform backend may need
corresponding changes to `[native].link_options`.
