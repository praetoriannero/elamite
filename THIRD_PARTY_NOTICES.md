# Third-party notices

The compiler depends on Rust crates recorded exactly in `Cargo.lock`. Their
copyright and license terms remain with their respective authors; this
repository's license does not replace those terms. A distribution must retain
the license files supplied by those crates and by the native libraries it
ships.

Generated native programs may link the Boehm-Demers-Weiser garbage collector
when managed storage is required. Its license and notices are supplied by the
installed collector package and must accompany any distribution that bundles
the collector.

The repository currently has `publish = false` and does not produce a bundled
third-party binary distribution. The release audit must be repeated if that
packaging policy changes.
