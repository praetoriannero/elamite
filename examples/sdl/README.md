# SDL demo

This package opens an SDL2 window and draws a small animated scene. Close the
window or press Escape to exit.

The example keeps SDL's C-only details in a small C99 shim. In particular,
`SDL_Event` is a C union, so Elamite sees only an opaque application handle and
four narrow functions. The shim validates its single application handle and
makes cleanup idempotent. `SdlDemo` contains all foreign calls inside `unsafe`
blocks and presents a safe Elamite interface to `main`.

Install the SDL2 development package first. On Debian or Ubuntu:

```sh
sudo apt install libsdl2-dev
```

On Fedora:

```sh
sudo dnf install SDL2-devel
```

Then run the example from the repository root:

```sh
cargo run -- run examples/sdl
```

The manifest passes the C shim and `-lSDL2` to the native compiler. It assumes
SDL2 headers and libraries are in the compiler's standard search paths.
