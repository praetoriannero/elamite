//! Structured compiler diagnostics: a stable category, a plain-language
//! message, an optional primary span, and related spans, per `ROADMAP.md` §2.3.

use crate::source::Span;

/// A stable, matchable diagnostic category. New variants may be added as
/// later milestones introduce new failure classes; an existing variant's
/// meaning must not change once tests depend on it (`ROADMAP.md` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// `elamite.toml` is missing, unreadable, malformed, or fails manifest
    /// validation (Milestone 1).
    ManifestInvalid,
    /// File-backed module discovery found an invalid path component or a
    /// module-path collision (Milestone 1).
    ModuleDiscoveryInvalid,
    /// The resolved package dependency graph is invalid, such as a cycle
    /// (Milestone 1).
    PackageGraphInvalid,
    /// Leading indentation or a dedent violates the four-space layout rules
    /// (Milestone 2).
    LexicalIndentation,
    /// An opening or closing grouping delimiter is missing or mismatched
    /// (Milestone 2).
    LexicalDelimiter,
    /// A numeric, string, character, or formatted-string literal is malformed
    /// (Milestone 2).
    LexicalLiteral,
    /// Source text contains a character that cannot begin any token
    /// (Milestone 2).
    LexicalCharacter,
    /// A token sequence does not form the required surface-language construct
    /// (Milestone 3).
    Syntax,
    /// Two declarations, imports, modules, members, or lexical bindings
    /// conflict in one namespace (Milestone 4).
    DeclarationConflict,
    /// A path or unqualified name cannot be resolved to a stable identity
    /// (Milestone 4).
    NameResolution,
    /// An item exists but is not accessible from the use or public signature
    /// being checked (Milestone 4).
    Visibility,
    /// A resolved type is ill-formed, has the wrong number of arguments, or
    /// participates in an invalid transparent-alias cycle (Milestone 5).
    TypeSystem,
    /// A literal cannot be represented by its contextual or suffixed concrete
    /// type (Milestone 5).
    LiteralType,
    /// An expression's type does not match what its context requires:
    /// operand, condition, cast, or return-type mismatches (Milestone 6).
    ExpressionType,
    /// A call has the wrong argument count or its callee is not callable
    /// (Milestone 6).
    Call,
    /// A struct, enum, tuple, or array literal has a missing, duplicate, or
    /// out-of-place field, or a mismatched element count (Milestone 6).
    Construction,
    /// An assignment, compound assignment, or reference formation targets a
    /// place that is not addressable or not mutable (Milestone 6).
    Place,
    /// A struct or enum containment cycle does not cross an explicit safe
    /// reference or raw-pointer indirection (Milestone 6).
    Containment,
    /// A pattern's shape does not match its scrutinee type, a struct/variant
    /// pattern has a missing, unknown, or duplicate field, an alternative
    /// pattern's arms bind different names, or a `match` arm is unreachable
    /// or the match is not exhaustive (Milestone 7).
    Pattern,
    /// `break`/`continue` appears outside a loop, or a non-unit function has
    /// a reachable path that does not return a value (Milestone 7).
    ControlFlow,
    /// An unsafe-only operation — raw-pointer dereference, raw-to-reference
    /// conversion, a pointee-changing cast, or a call to an unsafe or foreign
    /// function — appears outside an `unsafe:` block (Milestone 16).
    UnsafeContext,
    /// A raw-pointer access whose operand is an expression-local compile-time
    /// constant known to violate the null or alignment requirement
    /// (Milestone 16). This never propagates facts through bindings,
    /// assignments, branches, reachability, or calls.
    PointerValidity,
    /// Checked syntax cannot be represented by the current typed/control-flow
    /// IR boundary (Milestone 8).
    Lowering,
    /// A canonical type or control-flow operation has no valid C99
    /// representation in the selected backend (Milestone 8).
    CodeGeneration,
    /// The selected C compiler, linker, target, or output path failed while
    /// producing an artifact (Milestone 8).
    Toolchain,
}

/// One compiler diagnostic.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub category: Category,
    pub message: String,
    pub primary: Option<Span>,
    pub related: Vec<(Span, String)>,
}

impl Diagnostic {
    #[must_use]
    pub fn new(category: Category, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            primary: None,
            related: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_primary(mut self, span: Span) -> Self {
        self.primary = Some(span);
        self
    }

    #[must_use]
    pub fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related.push((span, message.into()));
        self
    }
}
