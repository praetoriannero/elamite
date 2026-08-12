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

/// One compiler-owned bodyless standard declaration and the irreducible
/// capability supplied by its backend lowering. These declarations are
/// intentionally private implementation details of shipped Elamite modules.
pub const NATIVE_DECLARATIONS: &[Intrinsic] = &[
    Intrinsic {
        path: "std.panic",
        reason: "process-fatal runtime trap entry with caller location",
    },
    Intrinsic {
        path: "std.trap",
        reason: "typed process-fatal runtime trap entry with caller location",
    },
    Intrinsic {
        path: "std.fs._view",
        reason: "borrowed view of opaque owned text stored by a source path",
    },
    Intrinsic {
        path: "std.fs._open",
        reason: "native file-handle acquisition",
    },
    Intrinsic {
        path: "std.fs._read_dir",
        reason: "native directory-stream acquisition",
    },
    Intrinsic {
        path: "std.fs._metadata",
        reason: "host filesystem metadata query",
    },
    Intrinsic {
        path: "std.fs._create_dir",
        reason: "host filesystem directory creation",
    },
    Intrinsic {
        path: "std.fs._remove_dir",
        reason: "host filesystem directory removal",
    },
    Intrinsic {
        path: "std.fs._remove_file",
        reason: "host filesystem entry removal",
    },
    Intrinsic {
        path: "std.fs._rename",
        reason: "host filesystem rename operation",
    },
    Intrinsic {
        path: "std.env._args",
        reason: "host process argument snapshot",
    },
    Intrinsic {
        path: "std.env._get",
        reason: "host process environment lookup",
    },
    Intrinsic {
        path: "std.env._current_dir",
        reason: "host current-directory query",
    },
    Intrinsic {
        path: "std.process._run",
        reason: "native child-process creation and pipe collection",
    },
    Intrinsic {
        path: "std.process._exit",
        reason: "immediate host process termination",
    },
    Intrinsic {
        path: "std.time._monotonic_now",
        reason: "host monotonic-clock read",
    },
    Intrinsic {
        path: "std.time._system_now",
        reason: "host wall-clock read",
    },
    Intrinsic {
        path: "std.testing.assert",
        reason: "structured assertion trap entry with caller location",
    },
    Intrinsic {
        path: "std.testing.fail",
        reason: "formatted structured assertion trap entry with caller location",
    },
    Intrinsic {
        path: "std.text._byte_len",
        reason: "opaque borrowed-text descriptor length",
    },
    Intrinsic {
        path: "std.text._next_scalar",
        reason: "validated UTF-8 decoding over an opaque text descriptor",
    },
    Intrinsic {
        path: "std.text._slice_bytes",
        reason: "checked UTF-8 view construction retaining owned backing",
    },
    Intrinsic {
        path: "std.text._string_view",
        reason: "borrowed view of opaque owned-text backing",
    },
    Intrinsic {
        path: "std.text._from_chars",
        reason: "owned-text allocation and UTF-8 scalar encoding",
    },
];

#[must_use]
pub fn native_declaration_reason(path: &str) -> Option<&'static str> {
    NATIVE_DECLARATIONS
        .iter()
        .find(|intrinsic| intrinsic.path == path)
        .map(|intrinsic| intrinsic.reason)
}

pub const ROOT_SOURCE: &str = include_str!("../stdlib/src/lib.elx");
pub const IO_SOURCE: &str = include_str!("../stdlib/src/io.elx");
pub const FS_SOURCE: &str = include_str!("../stdlib/src/fs.elx");
pub const ENV_SOURCE: &str = include_str!("../stdlib/src/env.elx");
pub const PROCESS_SOURCE: &str = include_str!("../stdlib/src/process.elx");
pub const TIME_SOURCE: &str = include_str!("../stdlib/src/time.elx");
pub const RANDOM_SOURCE: &str = include_str!("../stdlib/src/random.elx");
pub const ORDERING_SOURCE: &str = include_str!("../stdlib/src/ordering.elx");
pub const TEXT_SOURCE: &str = include_str!("../stdlib/src/text.elx");
pub const FFI_SOURCE: &str = include_str!("../stdlib/src/ffi.elx");
pub const TESTING_SOURCE: &str = include_str!("../stdlib/src/testing.elx");
pub const THREAD_SOURCE: &str = include_str!("../stdlib/src/thread.elx");
pub const SYNC_SOURCE: &str = include_str!("../stdlib/src/sync.elx");

/// Exact source-backed public declarations shipped by the compiler.
pub const SOURCE_DECLARATIONS: &[&str] = &[
    "std.Callable",
    "std.Clone",
    "std.Display",
    "std.Drop",
    "std.drop",
    "std.Iterator",
    "std.NumericError",
    "std.Option",
    "std.panic",
    "std.trap",
    "std.Result",
    "std.io.IoError",
    "std.fs.Path",
    "std.fs.FileType",
    "std.fs.Metadata",
    "std.fs.OpenMode",
    "std.fs.DirectoryEntry",
    "std.fs.open",
    "std.fs.read_dir",
    "std.fs.metadata",
    "std.fs.create_dir",
    "std.fs.remove_dir",
    "std.fs.remove_file",
    "std.fs.rename",
    "std.env.EnvError",
    "std.env.args",
    "std.env.get",
    "std.env.current_dir",
    "std.process.ProcessError",
    "std.process.ExitStatus",
    "std.process.Output",
    "std.process.run",
    "std.process.exit",
    "std.time.Duration",
    "std.time.Instant",
    "std.time.SystemTime",
    "std.time.monotonic_now",
    "std.time.system_now",
    "std.random.Generator",
    "std.ordering.Ordering",
    "std.ordering.compare",
    "std.ordering.sort",
    "std.ordering.binary_search",
    "std.ordering.binary_search_vec",
    "std.text.ParseError",
    "std.text.find",
    "std.text.contains",
    "std.text.split",
    "std.text.split_string",
    "std.text.trim",
    "std.text.trim_string",
    "std.text.to_lowercase",
    "std.text.to_uppercase",
    "std.text.parse_i64",
    "std.text.parse_u64",
    "std.text.parse_bool",
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
        reason: "revision-selected collection representation and operations",
    },
    Intrinsic {
        path: "Map",
        reason: "revision-selected collection representation and operations",
    },
    Intrinsic {
        path: "Set",
        reason: "revision-selected collection representation and operations",
    },
    Intrinsic {
        path: "Box",
        reason: "unique address-stable heap ownership and lowering hooks",
    },
    Intrinsic {
        path: "Shared",
        reason: "atomic explicit shared ownership and lowering hooks",
    },
    Intrinsic {
        path: "Weak",
        reason: "non-owning shared control-block identity and lowering hooks",
    },
    Intrinsic {
        path: "Store",
        reason: "homogeneous generational graph storage and lowering hooks",
    },
    Intrinsic {
        path: "Handle",
        reason: "target-width generational store identity representation",
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
        path: "Copy",
        reason: "compiler-controlled structural ownership capability",
    },
    Intrinsic {
        path: "Send",
        reason: "compiler-controlled structural thread-transfer capability",
    },
    Intrinsic {
        path: "Sync",
        reason: "compiler-controlled structural shared-thread capability",
    },
    Intrinsic {
        path: "Formatter",
        reason: "backend formatting-buffer representation",
    },
    Intrinsic {
        path: "Identity",
        reason: "stable-address identity representation",
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
        reason: "foreign retained-pointer compatibility representation",
    },
    Intrinsic {
        path: "std.ffi.ForeignRootMut",
        reason: "mutable foreign retained-pointer compatibility representation",
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
        path: "std.thread.ScopedThread",
        reason: "move-only scoped join identity and result storage",
    },
    Intrinsic {
        path: "std.thread.Scope",
        reason: "lexically bounded native child-thread registry",
    },
    Intrinsic {
        path: "std.thread.spawn",
        reason: "native thread creation and revision-selected callable transfer",
    },
    Intrinsic {
        path: "std.thread.scope",
        reason: "borrow-bounded native thread execution region",
    },
    Intrinsic {
        path: "std.fs.File",
        reason: "owned native file-handle representation",
    },
    Intrinsic {
        path: "std.fs.Directory",
        reason: "owned native directory-stream representation",
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
        path: "std.sync.MutexGuard",
        reason: "move-only lexical mutex ownership and borrowed protected access",
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
