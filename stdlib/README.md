# Compiler-shipped `std` package

The `.elx` files in `src/` are compiled into the Elamite compiler and pass
through the ordinary lexer, parser, resolver, type checker, and lowering
pipeline. `src/standard.rs` contains the reviewed inventory of entities that
remain intrinsic because Elamite source cannot yet express their runtime
representation or lowering hook.

Keep the manifest and source tree valid as an ordinary `lib` package. Moving an
entity out of the intrinsic catalog requires source declarations plus
behavioral and diagnostic compatibility tests.

Native concurrency follows `docs/spec.md` Section 10.4. Public declarations live
in `std.thread` and `std.sync`; only native representation, publication, thread,
queue, mutex, atomic, and collector-registration hooks remain intrinsic.
Thread environments, join results, and channel messages use ordinary shallow
copies. Synchronization safely publishes those immediate representations but
does not detach mutable backing or prevent later data races.

The intrinsic method surface is:

- `Thread[R].join() -> R` and `is_finished() -> bool`;
- `Sender[T].send(T)`, `try_send(T)`, and `close()`, plus
  `Receiver[T].receive()`, `try_receive()`, and `close()`;
- `Mutex[T].new(T)`, `read()`, `replace(T)`, and
  `update(fn(T) -> T)`;
- atomic `new`, `load`, `store`, `exchange`, and `compare_exchange`, with
  `fetch_add` and `fetch_sub` on `AtomicI32` and `AtomicUsize`.

Channel constructors and operation outcome types are declared in `std.sync`;
thread construction and `SpawnError` are declared in `std.thread`.
