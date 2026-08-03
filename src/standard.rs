//! Compiler-shipped standard-package sources and intrinsic inventory.
//!
//! Source-backed entities follow the ordinary lexer, parser, resolution, type,
//! and lowering paths. Intrinsic entities remain compiler-known only when the
//! initial language cannot express their representation or lowering contract.

/// One compiler-known entity and the reason it cannot yet be ordinary Elamite
/// source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intrinsic {
    pub path: &'static str,
    pub reason: &'static str,
}

pub const ROOT_SOURCE: &str = include_str!("../stdlib/src/lib.elx");
pub const IO_SOURCE: &str = include_str!("../stdlib/src/io.elx");
pub const FFI_SOURCE: &str = include_str!("../stdlib/src/ffi.elx");
pub const TESTING_SOURCE: &str = include_str!("../stdlib/src/testing.elx");
pub const THREAD_SOURCE: &str = include_str!("../stdlib/src/thread.elx");
pub const SYNC_SOURCE: &str = include_str!("../stdlib/src/sync.elx");

/// Exact source-backed public declarations shipped by the compiler.
pub const SOURCE_DECLARATIONS: &[&str] = &[
    "std.Callable",
    "std.Display",
    "std.NumericError",
    "std.Option",
    "std.panic",
    "std.trap",
    "std.Result",
    "std.io.IoError",
    "std.testing.RuntimeTrap",
    "std.testing.BuiltinTrap",
    "std.testing.assert",
    "std.testing.fail",
    "std.thread.SpawnError",
    "std.sync.SendError",
    "std.sync.TryReceiveError",
    "std.sync.TrySendError",
];

/// Exact compiler-known entity inventory.
///
/// Primitive numeric types share one representation/lowering reason but stay
/// listed individually so adding or removing a compiler-known spelling cannot
/// bypass the inventory test.
pub const INTRINSICS: &[Intrinsic] = &[
    Intrinsic {
        path: "bool",
        reason: "primitive value representation and operators",
    },
    Intrinsic {
        path: "char",
        reason: "primitive Unicode scalar representation",
    },
    Intrinsic {
        path: "i8",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "i16",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "i32",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "i64",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "i128",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "isize",
        reason: "target-pointer-width numeric representation",
    },
    Intrinsic {
        path: "u8",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "u16",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "u32",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "u64",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "u128",
        reason: "primitive numeric representation and checked operators",
    },
    Intrinsic {
        path: "usize",
        reason: "target-pointer-width numeric representation",
    },
    Intrinsic {
        path: "f32",
        reason: "primitive floating-point representation and operators",
    },
    Intrinsic {
        path: "f64",
        reason: "primitive floating-point representation and operators",
    },
    Intrinsic {
        path: "str",
        reason: "borrowed text representation and literal materialization",
    },
    Intrinsic {
        path: "String",
        reason: "owned text representation and copy/lowering hooks",
    },
    Intrinsic {
        path: "Vec",
        reason: "managed collection representation and operations",
    },
    Intrinsic {
        path: "Map",
        reason: "managed collection representation and operations",
    },
    Intrinsic {
        path: "Set",
        reason: "managed collection representation and operations",
    },
    Intrinsic {
        path: "Default",
        reason: "compiler-controlled structural derivation and primitive implementations",
    },
    Intrinsic {
        path: "PartialEq",
        reason: "operator selection and structural derivation",
    },
    Intrinsic {
        path: "Eq",
        reason: "compiler-controlled structural derivation",
    },
    Intrinsic {
        path: "PartialOrd",
        reason: "operator selection and structural derivation",
    },
    Intrinsic {
        path: "Ord",
        reason: "compiler-controlled structural derivation",
    },
    Intrinsic {
        path: "Hash",
        reason: "compiler-controlled structural derivation",
    },
    Intrinsic {
        path: "StableHash",
        reason: "compiler-inferred collection-key capability",
    },
    Intrinsic {
        path: "Formatter",
        reason: "backend formatting-buffer representation",
    },
    Intrinsic {
        path: "Identity",
        reason: "managed-address identity representation",
    },
    Intrinsic {
        path: "print",
        reason: "standard-output lowering hook",
    },
    Intrinsic {
        path: "println",
        reason: "standard-output lowering hook",
    },
    Intrinsic {
        path: "std.ffi.ForeignRoot",
        reason: "garbage-collector root registration",
    },
    Intrinsic {
        path: "std.ffi.ForeignRootMut",
        reason: "mutable garbage-collector root registration",
    },
    Intrinsic {
        path: "std.ffi.CVoid",
        reason: "opaque C void correspondence",
    },
    Intrinsic {
        path: "std.thread.Thread",
        reason: "native thread identity and cached-result representation",
    },
    Intrinsic {
        path: "std.thread.spawn",
        reason: "native thread creation and shallow callable publication",
    },
    Intrinsic {
        path: "std.sync.Sender",
        reason: "synchronized channel endpoint representation",
    },
    Intrinsic {
        path: "std.sync.Receiver",
        reason: "synchronized channel endpoint representation",
    },
    Intrinsic {
        path: "std.sync.Mutex",
        reason: "native synchronized shared-cell representation",
    },
    Intrinsic {
        path: "std.sync.AtomicBool",
        reason: "C99 sequentially consistent atomic-cell hook",
    },
    Intrinsic {
        path: "std.sync.AtomicI32",
        reason: "C99 sequentially consistent atomic-cell hook",
    },
    Intrinsic {
        path: "std.sync.AtomicUsize",
        reason: "target-width C99 sequentially consistent atomic-cell hook",
    },
    Intrinsic {
        path: "std.sync.channel",
        reason: "bounded native channel construction",
    },
    Intrinsic {
        path: "std.sync.unbounded_channel",
        reason: "unbounded native channel construction",
    },
];

#[must_use]
pub fn intrinsic_leaf_names() -> Vec<&'static str> {
    INTRINSICS
        .iter()
        .map(|intrinsic| {
            intrinsic
                .path
                .rsplit_once('.')
                .map_or(intrinsic.path, |(_, name)| name)
        })
        .collect()
}
