//! Versioned, compile-time-only `std.ast` façade.
//!
//! These values describe pre-resolution source structure without borrowing or
//! exposing [`crate::syntax::SyntaxNode`], resolver identities, inferred types,
//! target layout, or mutable compiler tables. Values are immutable and cheaply
//! cloneable; list transforms preserve their inputs and share contained nodes.

use std::fmt;
use std::sync::Arc;

use crate::config::SemanticRevision;
use crate::diagnostics::{Category, Diagnostic};
use crate::expansion::provenance::{OriginRange, ProvenanceTable};
use crate::ident::is_valid_identifier;

/// Exact ABI version of the compile-time structural syntax interface.
///
/// Public compile-time artifacts record this value and must be rejected when
/// it differs. Compatibility is intentionally exact until stabilization has
/// frozen rules for evolving the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AstInterfaceVersion {
    pub major: u16,
    pub minor: u16,
}

pub const INTERFACE_VERSION: AstInterfaceVersion = AstInterfaceVersion { major: 2, minor: 0 };
/// Interface selected by the accepted owned-model surface. It remains behind
/// the semantic-revision seam until final owned-model conformance.
pub const OWNED_INTERFACE_VERSION: AstInterfaceVersion = AstInterfaceVersion { major: 3, minor: 0 };

/// Stable public type inventory admitted by the initial façade.
pub const TYPE_NAMES: &[&str] = &[
    "std.ast.Attribute",
    "std.ast.EnumDefinition",
    "std.ast.EnumVariant",
    "std.ast.Expression",
    "std.ast.FieldDefinition",
    "std.ast.FunctionDefinition",
    "std.ast.GenericParameter",
    "std.ast.Identifier",
    "std.ast.Implementation",
    "std.ast.InherentImplementation",
    "std.ast.Item",
    "std.ast.ItemList",
    "std.ast.Member",
    "std.ast.MemberList",
    "std.ast.Origin",
    "std.ast.Parameter",
    "std.ast.Pattern",
    "std.ast.Statement",
    "std.ast.StatementList",
    "std.ast.StructDefinition",
    "std.ast.TypeSyntax",
];

/// Exact type inventory selected by the 3.0 owned-surface interface.
pub const OWNED_TYPE_NAMES: &[&str] = &[
    "std.ast.Attribute",
    "std.ast.ClosureCapture",
    "std.ast.ClosureCaptureMode",
    "std.ast.ClosureExpression",
    "std.ast.EnumDefinition",
    "std.ast.EnumVariant",
    "std.ast.Expression",
    "std.ast.FieldDefinition",
    "std.ast.FunctionDefinition",
    "std.ast.GenericParameter",
    "std.ast.Identifier",
    "std.ast.Implementation",
    "std.ast.InherentImplementation",
    "std.ast.Item",
    "std.ast.ItemList",
    "std.ast.Member",
    "std.ast.MemberList",
    "std.ast.Origin",
    "std.ast.Parameter",
    "std.ast.Pattern",
    "std.ast.Statement",
    "std.ast.StatementList",
    "std.ast.StructDefinition",
    "std.ast.TypeSyntax",
];

/// A negotiated view of the intrinsic interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstInterface {
    version: AstInterfaceVersion,
}

impl AstInterface {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            version: INTERFACE_VERSION,
        }
    }

    #[must_use]
    pub const fn for_semantic_revision(revision: SemanticRevision) -> Self {
        Self {
            version: match revision {
                SemanticRevision::V0_10 => INTERFACE_VERSION,
                SemanticRevision::V0_11 => OWNED_INTERFACE_VERSION,
            },
        }
    }

    /// Requires an exact version match. This is the same check used when a
    /// future compiled package imports public compile-time metadata.
    pub fn negotiate(required: AstInterfaceVersion) -> Result<Self, AstError> {
        Self::negotiate_for_semantic_revision(required, SemanticRevision::default())
    }

    pub fn negotiate_for_semantic_revision(
        required: AstInterfaceVersion,
        revision: SemanticRevision,
    ) -> Result<Self, AstError> {
        let provided = Self::for_semantic_revision(revision);
        if required == provided.version() {
            Ok(provided)
        } else {
            Err(AstError::VersionMismatch {
                required,
                provided: provided.version(),
            })
        }
    }

    #[must_use]
    pub const fn version(self) -> AstInterfaceVersion {
        self.version
    }

    #[must_use]
    pub const fn type_names(self) -> &'static [&'static str] {
        if self.version.major == OWNED_INTERFACE_VERSION.major {
            OWNED_TYPE_NAMES
        } else {
            TYPE_NAMES
        }
    }

    /// Creates the intrinsic builder used by one interpreter execution.
    /// Only the compiler can supply an origin handle.
    pub fn builder(self, origin: OriginHandle) -> AstBuilder {
        AstBuilder {
            interface: self,
            origin,
        }
    }

    /// Implements the non-returning `std.ast.error` intrinsic as a contained
    /// interpreter failure. The interpreter will stop the current execution
    /// and turn this value into a diagnostic.
    #[must_use]
    pub fn error(self, node: &impl HasOrigin, message: impl Into<String>) -> AstError {
        AstError::Reported {
            origin: node.origin(),
            message: message.into(),
        }
    }
}

/// An opaque location retained by an AST value.
///
/// There is intentionally no public constructor or physical-span accessor.
/// Compile-time code may pass this handle back to diagnostic intrinsics but
/// cannot fabricate a file location or call-site context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OriginHandle {
    range: OriginRange,
}

impl OriginHandle {
    pub(crate) const fn from_range(range: OriginRange) -> Self {
        Self { range }
    }

    pub(crate) const fn range(self) -> OriginRange {
        self.range
    }
}

/// Common provenance accessor implemented by every façade node.
pub trait HasOrigin {
    fn origin(&self) -> OriginHandle;
}

/// A recoverable failure from the versioned AST boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstError {
    VersionMismatch {
        required: AstInterfaceVersion,
        provided: AstInterfaceVersion,
    },
    InvalidIdentifier {
        text: String,
        origin: OriginHandle,
    },
    InvalidConstruction {
        message: String,
        origin: OriginHandle,
    },
    Reported {
        origin: OriginHandle,
        message: String,
    },
}

impl AstError {
    /// Converts an AST failure to an ordinary compiler diagnostic without
    /// projecting generated origins onto fabricated physical spans.
    #[must_use]
    pub fn diagnostic(&self, provenance: &ProvenanceTable) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(Category::CompileTime, self.to_string());
        let origin = match self {
            Self::VersionMismatch { .. } => return diagnostic,
            Self::InvalidIdentifier { origin, .. }
            | Self::InvalidConstruction { origin, .. }
            | Self::Reported { origin, .. } => *origin,
        };

        let range = origin.range();
        if let Some(span) = provenance.physical_range(range) {
            return diagnostic.with_primary(span);
        }

        let trace = provenance.trace(range.first);
        for (index, frame) in trace.expansions.iter().enumerate() {
            if let Some(span) = provenance.physical_span(frame.invocation) {
                let label = if index == 0 {
                    "compile-time invocation"
                } else {
                    "enclosing compile-time invocation"
                };
                diagnostic = diagnostic.with_related(span, label);
            }
            if let Some(span) = provenance.physical_span(frame.definition) {
                diagnostic = diagnostic.with_related(span, "compile-time definition");
            }
        }
        diagnostic
    }
}

impl fmt::Display for AstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionMismatch { required, provided } => write!(
                formatter,
                "std.ast interface version {}.{} is required, but this compiler provides {}.{}",
                required.major, required.minor, provided.major, provided.minor
            ),
            Self::InvalidIdentifier { text, .. } => {
                write!(formatter, "`{text}` is not a valid Elamite identifier")
            }
            Self::InvalidConstruction { message, .. } | Self::Reported { message, .. } => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for AstError {}

fn is_reserved(text: &str) -> bool {
    matches!(
        text,
        "as" | "attr"
            | "break"
            | "continue"
            | "defer"
            | "derive"
            | "else"
            | "enum"
            | "expect"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "macro"
            | "match"
            | "mod"
            | "null"
            | "pass"
            | "pub"
            | "quote"
            | "return"
            | "root"
            | "self"
            | "Self"
            | "struct"
            | "super"
            | "test"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "var"
            | "while"
    )
}

/// A validated source identifier with its syntax context and origin hidden.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identifier(Arc<IdentifierData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct IdentifierData {
    text: Arc<str>,
    origin: OriginHandle,
}

impl Identifier {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0.text
    }

    pub fn with_text(&self, text: &str) -> Result<Self, AstError> {
        validate_identifier(text, self.origin())?;
        Ok(Self(Arc::new(IdentifierData {
            text: Arc::from(text),
            origin: self.origin(),
        })))
    }
}

impl HasOrigin for Identifier {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

fn validate_identifier(text: &str, origin: OriginHandle) -> Result<(), AstError> {
    if is_valid_identifier(text) && !is_reserved(text) {
        Ok(())
    } else {
        Err(AstError::InvalidIdentifier {
            text: text.to_string(),
            origin,
        })
    }
}

/// One immutable written path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyntaxPath(Arc<[Identifier]>);

impl SyntaxPath {
    #[must_use]
    pub fn segments(&self) -> &[Identifier] {
        &self.0
    }

    #[must_use]
    pub fn display(&self) -> String {
        self.0
            .iter()
            .map(Identifier::text)
            .collect::<Vec<_>>()
            .join(".")
    }
}

impl HasOrigin for SyntaxPath {
    fn origin(&self) -> OriginHandle {
        self.0[0].origin()
    }
}

/// Source visibility, independent of later accessibility decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

/// Compile-time primitive accepted by `std.ast.literal`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LiteralValue {
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    /// Exact IEEE-754 binary64 bits, retaining NaN payloads deterministically.
    Float64(u64),
    Character(char),
    String(Arc<str>),
    Null,
}

impl From<&str> for LiteralValue {
    fn from(value: &str) -> Self {
        Self::String(Arc::from(value))
    }
}

impl From<String> for LiteralValue {
    fn from(value: String) -> Self {
        Self::String(Arc::from(value))
    }
}

impl From<bool> for LiteralValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<char> for LiteralValue {
    fn from(value: char) -> Self {
        Self::Character(value)
    }
}

impl From<i64> for LiteralValue {
    fn from(value: i64) -> Self {
        Self::Signed(i128::from(value))
    }
}

impl From<u64> for LiteralValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(u128::from(value))
    }
}

impl From<f64> for LiteralValue {
    fn from(value: f64) -> Self {
        Self::Float64(value.to_bits())
    }
}

macro_rules! ast_list {
    ($name:ident, $element:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
        pub struct $name(Arc<[$element]>);

        impl $name {
            #[must_use]
            pub fn empty() -> Self {
                Self::default()
            }

            #[must_use]
            pub fn single(value: $element) -> Self {
                Self(Arc::from([value]))
            }

            #[must_use]
            pub fn length(&self) -> usize {
                self.0.len()
            }

            #[must_use]
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }

            #[must_use]
            pub fn get(&self, index: usize) -> Option<&$element> {
                self.0.get(index)
            }

            pub fn iter(&self) -> std::slice::Iter<'_, $element> {
                self.0.iter()
            }

            #[must_use]
            pub fn push(&self, value: $element) -> Self {
                let mut values = self.0.to_vec();
                values.push(value);
                Self(Arc::from(values))
            }

            #[must_use]
            pub fn concat(&self, other: &Self) -> Self {
                let mut values = Vec::with_capacity(self.length() + other.length());
                values.extend(self.0.iter().cloned());
                values.extend(other.0.iter().cloned());
                Self(Arc::from(values))
            }
        }

        impl IntoIterator for $name {
            type Item = $element;
            type IntoIter = std::vec::IntoIter<$element>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.to_vec().into_iter()
            }
        }

        impl From<Vec<$element>> for $name {
            fn from(values: Vec<$element>) -> Self {
                Self(Arc::from(values))
            }
        }
    };
}

ast_list!(ExpressionList, Expression);
ast_list!(PatternList, Pattern);
ast_list!(TypeSyntaxList, TypeSyntax);
ast_list!(StatementList, Statement);
ast_list!(ItemList, Item);
ast_list!(MemberList, Member);
ast_list!(FieldList, FieldDefinition);
ast_list!(VariantList, EnumVariant);
ast_list!(ParameterList, Parameter);
ast_list!(GenericParameterList, GenericParameter);
ast_list!(AttributeList, Attribute);

/// Readable expression variants. Payloads remain façade values rather than
/// compiler syntax nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClosureCaptureMode {
    Value,
    SharedBorrow,
    MutableBorrow,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClosureCapture(Arc<ClosureCaptureData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct ClosureCaptureData {
    mode: ClosureCaptureMode,
    source: Identifier,
    alias: Option<Identifier>,
    origin: OriginHandle,
}

impl ClosureCapture {
    #[must_use]
    pub fn mode(&self) -> ClosureCaptureMode {
        self.0.mode
    }

    #[must_use]
    pub fn source(&self) -> &Identifier {
        &self.0.source
    }

    #[must_use]
    pub fn alias(&self) -> Option<&Identifier> {
        self.0.alias.as_ref()
    }
}

impl HasOrigin for ClosureCapture {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClosureExpression(Arc<ClosureExpressionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct ClosureExpressionData {
    captures: Arc<[ClosureCapture]>,
    parameters: ParameterList,
    return_type: TypeSyntax,
    body: StatementList,
    origin: OriginHandle,
}

impl ClosureExpression {
    #[must_use]
    pub fn captures(&self) -> &[ClosureCapture] {
        &self.0.captures
    }

    #[must_use]
    pub fn parameters(&self) -> &ParameterList {
        &self.0.parameters
    }

    #[must_use]
    pub fn return_type(&self) -> &TypeSyntax {
        &self.0.return_type
    }

    #[must_use]
    pub fn body(&self) -> &StatementList {
        &self.0.body
    }
}

impl HasOrigin for ClosureExpression {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpressionVariant {
    Identifier(Identifier),
    Literal(LiteralValue),
    Call(CallExpression),
    Member(MemberExpression),
    Tuple(ExpressionList),
    Borrow { mutable: bool, value: Expression },
    Closure(ClosureExpression),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Expression(Arc<ExpressionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct ExpressionData {
    variant: ExpressionVariant,
    origin: OriginHandle,
}

impl Expression {
    #[must_use]
    pub fn variant(&self) -> ExpressionVariant {
        self.0.variant.clone()
    }

    #[must_use]
    pub fn display(&self) -> String {
        match &self.0.variant {
            ExpressionVariant::Identifier(identifier) => identifier.text().to_string(),
            ExpressionVariant::Literal(value) => format_literal(value),
            ExpressionVariant::Call(call) => format!("{}(...)", call.callee().display()),
            ExpressionVariant::Member(member) => {
                format!("{}.{}", member.receiver().display(), member.name().text())
            }
            ExpressionVariant::Tuple(elements) => {
                let values = elements.iter().map(Self::display).collect::<Vec<_>>();
                format!("({})", values.join(", "))
            }
            ExpressionVariant::Borrow { mutable, value } => {
                format!("&{}{}", if *mutable { "var " } else { "" }, value.display())
            }
            ExpressionVariant::Closure(closure) => {
                let captures = closure
                    .captures()
                    .iter()
                    .map(|capture| {
                        let prefix = match capture.mode() {
                            ClosureCaptureMode::Value => "",
                            ClosureCaptureMode::SharedBorrow => "&",
                            ClosureCaptureMode::MutableBorrow => "&var ",
                        };
                        let alias = capture
                            .alias()
                            .map_or_else(String::new, |alias| format!(" as {}", alias.text()));
                        format!("{prefix}{}{alias}", capture.source().text())
                    })
                    .collect::<Vec<_>>();
                format!("fn[{}](...): ...", captures.join(", "))
            }
        }
    }
}

impl HasOrigin for Expression {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

fn format_literal(value: &LiteralValue) -> String {
    match value {
        LiteralValue::Bool(value) => value.to_string(),
        LiteralValue::Signed(value) => value.to_string(),
        LiteralValue::Unsigned(value) => value.to_string(),
        LiteralValue::Float64(bits) => f64::from_bits(*bits).to_string(),
        LiteralValue::Character(value) => format!("{value:?}"),
        LiteralValue::String(value) => format!("{value:?}"),
        LiteralValue::Null => "null".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallExpression(Arc<CallExpressionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct CallExpressionData {
    callee: Expression,
    arguments: ExpressionList,
    origin: OriginHandle,
}

impl CallExpression {
    #[must_use]
    pub fn callee(&self) -> &Expression {
        &self.0.callee
    }

    #[must_use]
    pub fn arguments(&self) -> &ExpressionList {
        &self.0.arguments
    }

    #[must_use]
    pub fn with_callee(&self, callee: Expression) -> Self {
        Self(Arc::new(CallExpressionData {
            callee,
            arguments: self.0.arguments.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_arguments(&self, arguments: ExpressionList) -> Self {
        Self(Arc::new(CallExpressionData {
            callee: self.0.callee.clone(),
            arguments,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for CallExpression {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberExpression(Arc<MemberExpressionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct MemberExpressionData {
    receiver: Expression,
    name: Identifier,
    origin: OriginHandle,
}

impl MemberExpression {
    #[must_use]
    pub fn receiver(&self) -> &Expression {
        &self.0.receiver
    }

    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn with_receiver(&self, receiver: Expression) -> Self {
        Self(Arc::new(MemberExpressionData {
            receiver,
            name: self.0.name.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(MemberExpressionData {
            receiver: self.0.receiver.clone(),
            name,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for MemberExpression {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PatternVariant {
    Wildcard,
    Binding(Identifier),
    Literal(LiteralValue),
    Tuple(PatternList),
    Variant {
        path: SyntaxPath,
        fields: PatternList,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pattern(Arc<PatternData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct PatternData {
    variant: PatternVariant,
    origin: OriginHandle,
}

impl Pattern {
    #[must_use]
    pub fn variant(&self) -> PatternVariant {
        self.0.variant.clone()
    }
}

impl HasOrigin for Pattern {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeSyntaxVariant {
    Named {
        path: SyntaxPath,
        arguments: TypeSyntaxList,
    },
    Tuple(TypeSyntaxList),
    Reference {
        mutable: bool,
        target: TypeSyntax,
    },
    Slice {
        mutable: bool,
        element: TypeSyntax,
    },
    Array {
        element: TypeSyntax,
        length: Expression,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeSyntax(Arc<TypeSyntaxData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct TypeSyntaxData {
    variant: TypeSyntaxVariant,
    origin: OriginHandle,
}

impl TypeSyntax {
    #[must_use]
    pub fn variant(&self) -> TypeSyntaxVariant {
        self.0.variant.clone()
    }

    #[must_use]
    pub fn display(&self) -> String {
        match &self.0.variant {
            TypeSyntaxVariant::Named { path, arguments } => {
                if arguments.is_empty() {
                    path.display()
                } else {
                    let values = arguments.iter().map(Self::display).collect::<Vec<_>>();
                    format!("{}[{}]", path.display(), values.join(", "))
                }
            }
            TypeSyntaxVariant::Tuple(elements) => {
                let values = elements.iter().map(Self::display).collect::<Vec<_>>();
                format!("({})", values.join(", "))
            }
            TypeSyntaxVariant::Reference { mutable, target } => {
                format!(
                    "&{}{}",
                    if *mutable { "var " } else { "" },
                    target.display()
                )
            }
            TypeSyntaxVariant::Slice { mutable, element } => {
                format!(
                    "[{}{}]",
                    if *mutable { "var " } else { "" },
                    element.display()
                )
            }
            TypeSyntaxVariant::Array { element, length } => {
                format!("[{}; {}]", element.display(), length.display())
            }
        }
    }
}

impl HasOrigin for TypeSyntax {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StatementVariant {
    Expression(Expression),
    Return(Option<Expression>),
    Let {
        pattern: Pattern,
        annotation: Option<TypeSyntax>,
        value: Expression,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Statement(Arc<StatementData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct StatementData {
    variant: StatementVariant,
    origin: OriginHandle,
}

impl Statement {
    #[must_use]
    pub fn variant(&self) -> StatementVariant {
        self.0.variant.clone()
    }
}

impl HasOrigin for Statement {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

/// Common written structure carried by module-level definitions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DefinitionMetadata(Arc<DefinitionMetadataData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct DefinitionMetadataData {
    name: Identifier,
    visibility: Visibility,
    generics: GenericParameterList,
    documentation: Arc<[Arc<str>]>,
    attributes: AttributeList,
    origin: OriginHandle,
}

impl DefinitionMetadata {
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn visibility(&self) -> Visibility {
        self.0.visibility
    }

    #[must_use]
    pub fn generics(&self) -> &GenericParameterList {
        &self.0.generics
    }

    #[must_use]
    pub fn documentation(&self) -> &[Arc<str>] {
        &self.0.documentation
    }

    #[must_use]
    pub fn attributes(&self) -> &AttributeList {
        &self.0.attributes
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(DefinitionMetadataData {
            name,
            visibility: self.0.visibility,
            generics: self.0.generics.clone(),
            documentation: self.0.documentation.clone(),
            attributes: self.0.attributes.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_visibility(&self, visibility: Visibility) -> Self {
        Self(Arc::new(DefinitionMetadataData {
            name: self.0.name.clone(),
            visibility,
            generics: self.0.generics.clone(),
            documentation: self.0.documentation.clone(),
            attributes: self.0.attributes.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_generics(&self, generics: GenericParameterList) -> Self {
        Self(Arc::new(DefinitionMetadataData {
            name: self.0.name.clone(),
            visibility: self.0.visibility,
            generics,
            documentation: self.0.documentation.clone(),
            attributes: self.0.attributes.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_documentation(&self, documentation: Vec<Arc<str>>) -> Self {
        Self(Arc::new(DefinitionMetadataData {
            name: self.0.name.clone(),
            visibility: self.0.visibility,
            generics: self.0.generics.clone(),
            documentation: Arc::from(documentation),
            attributes: self.0.attributes.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_attributes(&self, attributes: AttributeList) -> Self {
        Self(Arc::new(DefinitionMetadataData {
            name: self.0.name.clone(),
            visibility: self.0.visibility,
            generics: self.0.generics.clone(),
            documentation: self.0.documentation.clone(),
            attributes,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for DefinitionMetadata {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attribute(Arc<AttributeData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct AttributeData {
    path: SyntaxPath,
    arguments: ExpressionList,
    origin: OriginHandle,
}

impl Attribute {
    #[must_use]
    pub fn path(&self) -> &SyntaxPath {
        &self.0.path
    }

    #[must_use]
    pub fn arguments(&self) -> &ExpressionList {
        &self.0.arguments
    }

    #[must_use]
    pub fn with_path(&self, path: SyntaxPath) -> Self {
        Self(Arc::new(AttributeData {
            path,
            arguments: self.0.arguments.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_arguments(&self, arguments: ExpressionList) -> Self {
        Self(Arc::new(AttributeData {
            path: self.0.path.clone(),
            arguments,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for Attribute {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParameter(Arc<GenericParameterData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct GenericParameterData {
    name: Identifier,
    bounds: TypeSyntaxList,
    origin: OriginHandle,
}

impl GenericParameter {
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn bounds(&self) -> &TypeSyntaxList {
        &self.0.bounds
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(GenericParameterData {
            name,
            bounds: self.0.bounds.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_bounds(&self, bounds: TypeSyntaxList) -> Self {
        Self(Arc::new(GenericParameterData {
            name: self.0.name.clone(),
            bounds,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for GenericParameter {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parameter(Arc<ParameterData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct ParameterData {
    name: Identifier,
    type_syntax: TypeSyntax,
    origin: OriginHandle,
}

impl Parameter {
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn type_syntax(&self) -> &TypeSyntax {
        &self.0.type_syntax
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(ParameterData {
            name,
            type_syntax: self.0.type_syntax.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_type_syntax(&self, type_syntax: TypeSyntax) -> Self {
        Self(Arc::new(ParameterData {
            name: self.0.name.clone(),
            type_syntax,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for Parameter {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldDefinition(Arc<FieldDefinitionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct FieldDefinitionData {
    name: Identifier,
    visibility: Visibility,
    type_syntax: TypeSyntax,
    origin: OriginHandle,
}

impl FieldDefinition {
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn visibility(&self) -> Visibility {
        self.0.visibility
    }

    #[must_use]
    pub fn type_syntax(&self) -> &TypeSyntax {
        &self.0.type_syntax
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(FieldDefinitionData {
            name,
            visibility: self.0.visibility,
            type_syntax: self.0.type_syntax.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_visibility(&self, visibility: Visibility) -> Self {
        Self(Arc::new(FieldDefinitionData {
            name: self.0.name.clone(),
            visibility,
            type_syntax: self.0.type_syntax.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_type_syntax(&self, type_syntax: TypeSyntax) -> Self {
        Self(Arc::new(FieldDefinitionData {
            name: self.0.name.clone(),
            visibility: self.0.visibility,
            type_syntax,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for FieldDefinition {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumVariant(Arc<EnumVariantData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct EnumVariantData {
    name: Identifier,
    fields: FieldList,
    origin: OriginHandle,
}

impl EnumVariant {
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.0.name
    }

    #[must_use]
    pub fn fields(&self) -> &FieldList {
        &self.0.fields
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        Self(Arc::new(EnumVariantData {
            name,
            fields: self.0.fields.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_fields(&self, fields: FieldList) -> Self {
        Self(Arc::new(EnumVariantData {
            name: self.0.name.clone(),
            fields,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for EnumVariant {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionDefinition(Arc<FunctionDefinitionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct FunctionDefinitionData {
    metadata: DefinitionMetadata,
    parameters: ParameterList,
    return_type: TypeSyntax,
    body: StatementList,
    origin: OriginHandle,
}

impl FunctionDefinition {
    #[must_use]
    pub fn metadata(&self) -> &DefinitionMetadata {
        &self.0.metadata
    }

    #[must_use]
    pub fn name(&self) -> &Identifier {
        self.0.metadata.name()
    }

    #[must_use]
    pub fn parameters(&self) -> &ParameterList {
        &self.0.parameters
    }

    #[must_use]
    pub fn return_type(&self) -> &TypeSyntax {
        &self.0.return_type
    }

    #[must_use]
    pub fn body(&self) -> &StatementList {
        &self.0.body
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        self.with_metadata(self.0.metadata.with_name(name))
    }

    #[must_use]
    pub fn with_visibility(&self, visibility: Visibility) -> Self {
        self.with_metadata(self.0.metadata.with_visibility(visibility))
    }

    #[must_use]
    pub fn with_metadata(&self, metadata: DefinitionMetadata) -> Self {
        Self(Arc::new(FunctionDefinitionData {
            metadata,
            parameters: self.0.parameters.clone(),
            return_type: self.0.return_type.clone(),
            body: self.0.body.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_parameters(&self, parameters: ParameterList) -> Self {
        Self(Arc::new(FunctionDefinitionData {
            metadata: self.0.metadata.clone(),
            parameters,
            return_type: self.0.return_type.clone(),
            body: self.0.body.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_return_type(&self, return_type: TypeSyntax) -> Self {
        Self(Arc::new(FunctionDefinitionData {
            metadata: self.0.metadata.clone(),
            parameters: self.0.parameters.clone(),
            return_type,
            body: self.0.body.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_body(&self, body: StatementList) -> Self {
        Self(Arc::new(FunctionDefinitionData {
            metadata: self.0.metadata.clone(),
            parameters: self.0.parameters.clone(),
            return_type: self.0.return_type.clone(),
            body,
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for FunctionDefinition {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructDefinition(Arc<StructDefinitionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct StructDefinitionData {
    metadata: DefinitionMetadata,
    members: MemberList,
    origin: OriginHandle,
}

impl StructDefinition {
    #[must_use]
    pub fn metadata(&self) -> &DefinitionMetadata {
        &self.0.metadata
    }

    #[must_use]
    pub fn name(&self) -> &Identifier {
        self.0.metadata.name()
    }

    #[must_use]
    pub fn fields(&self) -> FieldList {
        FieldList::from(
            self.0
                .members
                .iter()
                .filter_map(|member| match member {
                    Member::Field(field) => Some(field.clone()),
                    Member::Function(_) => None,
                })
                .collect::<Vec<_>>(),
        )
    }

    #[must_use]
    pub fn members(&self) -> &MemberList {
        &self.0.members
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        self.with_metadata(self.0.metadata.with_name(name))
    }

    #[must_use]
    pub fn with_visibility(&self, visibility: Visibility) -> Self {
        self.with_metadata(self.0.metadata.with_visibility(visibility))
    }

    #[must_use]
    pub fn with_metadata(&self, metadata: DefinitionMetadata) -> Self {
        Self(Arc::new(StructDefinitionData {
            metadata,
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_fields(&self, fields: FieldList) -> Self {
        let functions = self.0.members.iter().filter_map(|member| match member {
            Member::Field(_) => None,
            Member::Function(function) => Some(Member::Function(function.clone())),
        });
        let members = fields
            .iter()
            .cloned()
            .map(Member::Field)
            .chain(functions)
            .collect::<Vec<_>>();
        Self(Arc::new(StructDefinitionData {
            metadata: self.0.metadata.clone(),
            members: MemberList::from(members),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_members(&self, members: MemberList) -> Self {
        Self(Arc::new(StructDefinitionData {
            metadata: self.0.metadata.clone(),
            members,
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn type_syntax(&self) -> TypeSyntax {
        TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Named {
                path: SyntaxPath(Arc::from([self.name().clone()])),
                arguments: TypeSyntaxList::empty(),
            },
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for StructDefinition {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumDefinition(Arc<EnumDefinitionData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct EnumDefinitionData {
    metadata: DefinitionMetadata,
    variants: VariantList,
    members: MemberList,
    origin: OriginHandle,
}

impl EnumDefinition {
    #[must_use]
    pub fn metadata(&self) -> &DefinitionMetadata {
        &self.0.metadata
    }

    #[must_use]
    pub fn name(&self) -> &Identifier {
        self.0.metadata.name()
    }

    #[must_use]
    pub fn variants(&self) -> &VariantList {
        &self.0.variants
    }

    #[must_use]
    pub fn members(&self) -> &MemberList {
        &self.0.members
    }

    #[must_use]
    pub fn with_name(&self, name: Identifier) -> Self {
        self.with_metadata(self.0.metadata.with_name(name))
    }

    #[must_use]
    pub fn with_visibility(&self, visibility: Visibility) -> Self {
        self.with_metadata(self.0.metadata.with_visibility(visibility))
    }

    #[must_use]
    pub fn with_metadata(&self, metadata: DefinitionMetadata) -> Self {
        Self(Arc::new(EnumDefinitionData {
            metadata,
            variants: self.0.variants.clone(),
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_variants(&self, variants: VariantList) -> Self {
        Self(Arc::new(EnumDefinitionData {
            metadata: self.0.metadata.clone(),
            variants,
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_members(&self, members: MemberList) -> Self {
        Self(Arc::new(EnumDefinitionData {
            metadata: self.0.metadata.clone(),
            variants: self.0.variants.clone(),
            members,
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn type_syntax(&self) -> TypeSyntax {
        TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Named {
                path: SyntaxPath(Arc::from([self.name().clone()])),
                arguments: TypeSyntaxList::empty(),
            },
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for EnumDefinition {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Implementation(Arc<ImplementationData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct ImplementationData {
    trait_type: TypeSyntax,
    target_type: TypeSyntax,
    members: MemberList,
    origin: OriginHandle,
}

impl Implementation {
    #[must_use]
    pub fn trait_type(&self) -> &TypeSyntax {
        &self.0.trait_type
    }

    #[must_use]
    pub fn target_type(&self) -> &TypeSyntax {
        &self.0.target_type
    }

    #[must_use]
    pub fn members(&self) -> &MemberList {
        &self.0.members
    }

    #[must_use]
    pub fn with_members(&self, members: MemberList) -> Self {
        Self(Arc::new(ImplementationData {
            trait_type: self.0.trait_type.clone(),
            target_type: self.0.target_type.clone(),
            members,
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_trait_type(&self, trait_type: TypeSyntax) -> Self {
        Self(Arc::new(ImplementationData {
            trait_type,
            target_type: self.0.target_type.clone(),
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_target_type(&self, target_type: TypeSyntax) -> Self {
        Self(Arc::new(ImplementationData {
            trait_type: self.0.trait_type.clone(),
            target_type,
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for Implementation {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InherentImplementation(Arc<InherentImplementationData>);

#[derive(Debug, PartialEq, Eq, Hash)]
struct InherentImplementationData {
    target_type: TypeSyntax,
    members: MemberList,
    origin: OriginHandle,
}

impl InherentImplementation {
    #[must_use]
    pub fn target_type(&self) -> &TypeSyntax {
        &self.0.target_type
    }

    #[must_use]
    pub fn members(&self) -> &MemberList {
        &self.0.members
    }

    #[must_use]
    pub fn with_members(&self, members: MemberList) -> Self {
        Self(Arc::new(InherentImplementationData {
            target_type: self.0.target_type.clone(),
            members,
            origin: self.origin(),
        }))
    }

    #[must_use]
    pub fn with_target_type(&self, target_type: TypeSyntax) -> Self {
        Self(Arc::new(InherentImplementationData {
            target_type,
            members: self.0.members.clone(),
            origin: self.origin(),
        }))
    }
}

impl HasOrigin for InherentImplementation {
    fn origin(&self) -> OriginHandle {
        self.0.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Member {
    Field(FieldDefinition),
    Function(FunctionDefinition),
}

impl HasOrigin for Member {
    fn origin(&self) -> OriginHandle {
        match self {
            Self::Field(value) => value.origin(),
            Self::Function(value) => value.origin(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    Struct(StructDefinition),
    Enum(EnumDefinition),
    Function(FunctionDefinition),
    Implementation(Implementation),
    InherentImplementation(InherentImplementation),
}

impl HasOrigin for Item {
    fn origin(&self) -> OriginHandle {
        match self {
            Self::Struct(value) => value.origin(),
            Self::Enum(value) => value.origin(),
            Self::Function(value) => value.origin(),
            Self::Implementation(value) => value.origin(),
            Self::InherentImplementation(value) => value.origin(),
        }
    }
}

/// Validating constructors evaluated as `std.ast` intrinsics.
#[derive(Debug, Clone, Copy)]
pub struct AstBuilder {
    interface: AstInterface,
    origin: OriginHandle,
}

impl AstBuilder {
    #[must_use]
    pub const fn interface(self) -> AstInterface {
        self.interface
    }

    pub fn identifier(self, text: &str) -> Result<Identifier, AstError> {
        validate_identifier(text, self.origin)?;
        Ok(Identifier(Arc::new(IdentifierData {
            text: Arc::from(text),
            origin: self.origin,
        })))
    }

    pub fn path(self, segments: Vec<Identifier>) -> Result<SyntaxPath, AstError> {
        if segments.is_empty() {
            return Err(AstError::InvalidConstruction {
                message: "an AST path must contain at least one identifier".to_string(),
                origin: self.origin,
            });
        }
        Ok(SyntaxPath(Arc::from(segments)))
    }

    #[must_use]
    pub fn literal(self, value: impl Into<LiteralValue>) -> Expression {
        Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Literal(value.into()),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn identifier_expression(self, identifier: Identifier) -> Expression {
        Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Identifier(identifier),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn call(self, callee: Expression, arguments: ExpressionList) -> Expression {
        Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Call(CallExpression(Arc::new(CallExpressionData {
                callee,
                arguments,
                origin: self.origin,
            }))),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn member(self, receiver: Expression, name: Identifier) -> Expression {
        Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Member(MemberExpression(Arc::new(MemberExpressionData {
                receiver,
                name,
                origin: self.origin,
            }))),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn tuple_expression(self, elements: ExpressionList) -> Expression {
        Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Tuple(elements),
            origin: self.origin,
        }))
    }

    pub fn borrow_expression(
        self,
        mutable: bool,
        value: Expression,
    ) -> Result<Expression, AstError> {
        self.require_owned_surface()?;
        Ok(Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Borrow { mutable, value },
            origin: self.origin,
        })))
    }

    pub fn closure_capture(
        self,
        mode: ClosureCaptureMode,
        source: Identifier,
        alias: Option<Identifier>,
    ) -> Result<ClosureCapture, AstError> {
        self.require_owned_surface()?;
        Ok(ClosureCapture(Arc::new(ClosureCaptureData {
            mode,
            source,
            alias,
            origin: self.origin,
        })))
    }

    pub fn closure_expression(
        self,
        captures: Vec<ClosureCapture>,
        parameters: ParameterList,
        return_type: TypeSyntax,
        body: StatementList,
    ) -> Result<Expression, AstError> {
        self.require_owned_surface()?;
        Ok(Expression(Arc::new(ExpressionData {
            variant: ExpressionVariant::Closure(ClosureExpression(Arc::new(
                ClosureExpressionData {
                    captures: Arc::from(captures),
                    parameters,
                    return_type,
                    body,
                    origin: self.origin,
                },
            ))),
            origin: self.origin,
        })))
    }

    #[must_use]
    pub fn wildcard_pattern(self) -> Pattern {
        Pattern(Arc::new(PatternData {
            variant: PatternVariant::Wildcard,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn binding_pattern(self, name: Identifier) -> Pattern {
        Pattern(Arc::new(PatternData {
            variant: PatternVariant::Binding(name),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn literal_pattern(self, value: LiteralValue) -> Pattern {
        Pattern(Arc::new(PatternData {
            variant: PatternVariant::Literal(value),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn tuple_pattern(self, elements: PatternList) -> Pattern {
        Pattern(Arc::new(PatternData {
            variant: PatternVariant::Tuple(elements),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn variant_pattern(self, path: SyntaxPath, fields: PatternList) -> Pattern {
        Pattern(Arc::new(PatternData {
            variant: PatternVariant::Variant { path, fields },
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn named_type(self, path: SyntaxPath, arguments: TypeSyntaxList) -> TypeSyntax {
        TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Named { path, arguments },
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn tuple_type(self, elements: TypeSyntaxList) -> TypeSyntax {
        TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Tuple(elements),
            origin: self.origin,
        }))
    }

    fn require_owned_surface(self) -> Result<(), AstError> {
        if self.interface.version() == OWNED_INTERFACE_VERSION {
            Ok(())
        } else {
            Err(AstError::InvalidConstruction {
                message: "this AST form requires the owned-model std.ast interface".to_string(),
                origin: self.origin,
            })
        }
    }

    pub fn reference_type(self, mutable: bool, target: TypeSyntax) -> Result<TypeSyntax, AstError> {
        self.require_owned_surface()?;
        Ok(TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Reference { mutable, target },
            origin: self.origin,
        })))
    }

    pub fn slice_type(self, mutable: bool, element: TypeSyntax) -> Result<TypeSyntax, AstError> {
        self.require_owned_surface()?;
        Ok(TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Slice { mutable, element },
            origin: self.origin,
        })))
    }

    pub fn array_type(
        self,
        element: TypeSyntax,
        length: Expression,
    ) -> Result<TypeSyntax, AstError> {
        self.require_owned_surface()?;
        Ok(TypeSyntax(Arc::new(TypeSyntaxData {
            variant: TypeSyntaxVariant::Array { element, length },
            origin: self.origin,
        })))
    }

    #[must_use]
    pub fn expression_statement(self, expression: Expression) -> Statement {
        Statement(Arc::new(StatementData {
            variant: StatementVariant::Expression(expression),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn return_statement(self, expression: Option<Expression>) -> Statement {
        Statement(Arc::new(StatementData {
            variant: StatementVariant::Return(expression),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn let_statement(
        self,
        pattern: Pattern,
        annotation: Option<TypeSyntax>,
        value: Expression,
    ) -> Statement {
        Statement(Arc::new(StatementData {
            variant: StatementVariant::Let {
                pattern,
                annotation,
                value,
            },
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn metadata(self, name: Identifier) -> DefinitionMetadata {
        DefinitionMetadata(Arc::new(DefinitionMetadataData {
            name,
            visibility: Visibility::Private,
            generics: GenericParameterList::empty(),
            documentation: Arc::from([]),
            attributes: AttributeList::empty(),
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn attribute(self, path: SyntaxPath, arguments: ExpressionList) -> Attribute {
        Attribute(Arc::new(AttributeData {
            path,
            arguments,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn generic_parameter(self, name: Identifier, bounds: TypeSyntaxList) -> GenericParameter {
        GenericParameter(Arc::new(GenericParameterData {
            name,
            bounds,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn parameter(self, name: Identifier, type_syntax: TypeSyntax) -> Parameter {
        Parameter(Arc::new(ParameterData {
            name,
            type_syntax,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn field(
        self,
        name: Identifier,
        visibility: Visibility,
        type_syntax: TypeSyntax,
    ) -> FieldDefinition {
        FieldDefinition(Arc::new(FieldDefinitionData {
            name,
            visibility,
            type_syntax,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn variant(self, name: Identifier, fields: FieldList) -> EnumVariant {
        EnumVariant(Arc::new(EnumVariantData {
            name,
            fields,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn function(
        self,
        metadata: DefinitionMetadata,
        parameters: ParameterList,
        return_type: TypeSyntax,
        body: StatementList,
    ) -> FunctionDefinition {
        FunctionDefinition(Arc::new(FunctionDefinitionData {
            metadata,
            parameters,
            return_type,
            body,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn struct_definition(
        self,
        metadata: DefinitionMetadata,
        members: MemberList,
    ) -> StructDefinition {
        StructDefinition(Arc::new(StructDefinitionData {
            metadata,
            members,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn enum_definition(
        self,
        metadata: DefinitionMetadata,
        variants: VariantList,
        members: MemberList,
    ) -> EnumDefinition {
        EnumDefinition(Arc::new(EnumDefinitionData {
            metadata,
            variants,
            members,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn implementation(
        self,
        trait_type: TypeSyntax,
        target_type: TypeSyntax,
        members: MemberList,
    ) -> Implementation {
        Implementation(Arc::new(ImplementationData {
            trait_type,
            target_type,
            members,
            origin: self.origin,
        }))
    }

    #[must_use]
    pub fn inherent_implementation(
        self,
        target_type: TypeSyntax,
        members: MemberList,
    ) -> InherentImplementation {
        InherentImplementation(Arc::new(InherentImplementationData {
            target_type,
            members,
            origin: self.origin,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use crate::expansion::provenance::GeneratedSource;
    use crate::source::{SourceManager, Span};

    use super::*;

    fn fixture() -> (AstBuilder, ProvenanceTable) {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("ast.elx"), "source".to_string());
        let mut provenance = ProvenanceTable::new();
        let origin = provenance.register_physical(Span::new(file, 0, 6));
        let handle = OriginHandle::from_range(OriginRange::new(origin, origin));
        (AstInterface::current().builder(handle), provenance)
    }

    fn owned_fixture() -> (AstBuilder, ProvenanceTable) {
        let (builder, provenance) = fixture();
        (
            AstInterface::for_semantic_revision(SemanticRevision::V0_11).builder(builder.origin),
            provenance,
        )
    }

    fn named_type(builder: AstBuilder, name: &str) -> TypeSyntax {
        let identifier = builder.identifier(name).unwrap();
        let path = builder.path(vec![identifier]).unwrap();
        builder.named_type(path, TypeSyntaxList::empty())
    }

    #[test]
    fn interface_versions_are_exact_and_inventory_is_stable() {
        assert_eq!(AstInterface::current().version(), INTERFACE_VERSION);
        assert_eq!(
            AstInterface::for_semantic_revision(SemanticRevision::V0_11).version(),
            OWNED_INTERFACE_VERSION
        );
        assert!(
            AstInterface::negotiate_for_semantic_revision(
                OWNED_INTERFACE_VERSION,
                SemanticRevision::V0_11,
            )
            .is_ok()
        );
        assert!(AstInterface::negotiate(INTERFACE_VERSION).is_ok());
        assert!(matches!(
            AstInterface::negotiate(AstInterfaceVersion { major: 1, minor: 1 }),
            Err(AstError::VersionMismatch { .. })
        ));
        let retired = AstInterface::negotiate(AstInterfaceVersion { major: 1, minor: 0 })
            .expect_err("the frozen 1.0 interface is not reinterpreted as 2.0");
        assert_eq!(
            retired.to_string(),
            "std.ast interface version 1.0 is required, but this compiler provides 2.0"
        );
        assert_eq!(TYPE_NAMES.len(), 21);
        assert!(TYPE_NAMES.windows(2).all(|names| names[0] < names[1]));
        assert_eq!(
            AstInterface::for_semantic_revision(SemanticRevision::V0_11)
                .type_names()
                .len(),
            24
        );
        assert!(OWNED_TYPE_NAMES.windows(2).all(|names| names[0] < names[1]));
    }

    #[test]
    fn identifiers_and_paths_validate_without_exposing_origins() {
        let (builder, provenance) = fixture();
        let name = builder.identifier("Entity").unwrap();
        assert_eq!(name.text(), "Entity");
        assert_eq!(name.with_text("Renamed").unwrap().text(), "Renamed");
        let invalid = builder.identifier("2bad").unwrap_err();
        assert!(matches!(invalid, AstError::InvalidIdentifier { .. }));
        assert!(invalid.diagnostic(&provenance).primary.is_some());
        assert!(matches!(
            builder.identifier("struct"),
            Err(AstError::InvalidIdentifier { .. })
        ));
        assert!(matches!(
            builder.path(Vec::new()),
            Err(AstError::InvalidConstruction { .. })
        ));
    }

    #[test]
    fn expressions_types_patterns_and_statements_have_structural_variants() {
        let (builder, _) = fixture();
        let callee_name = builder.identifier("load").unwrap();
        let callee = builder.identifier_expression(callee_name);
        let argument = builder.literal(LiteralValue::Unsigned(42));
        let call = builder.call(callee, ExpressionList::single(argument));
        let ExpressionVariant::Call(call_data) = call.variant() else {
            panic!("expected call")
        };
        assert_eq!(call_data.callee().display(), "load");
        assert_eq!(call_data.arguments().length(), 1);

        let tuple = builder.tuple_pattern(PatternList::single(builder.wildcard_pattern()));
        assert!(matches!(tuple.variant(), PatternVariant::Tuple(_)));
        let type_syntax = named_type(builder, "u64");
        assert_eq!(type_syntax.display(), "u64");
        let statement = builder.return_statement(Some(call));
        assert!(matches!(
            statement.variant(),
            StatementVariant::Return(Some(_))
        ));
    }

    #[test]
    fn every_admitted_expression_pattern_type_and_statement_variant_is_constructible() {
        let (builder, _) = fixture();
        let name = builder.identifier("value").unwrap();
        let path = builder.path(vec![name.clone()]).unwrap();
        let identifier = builder.identifier_expression(name.clone());
        let literal = builder.literal(1_u64);
        let call = builder.call(identifier.clone(), ExpressionList::single(literal.clone()));
        let member = builder.member(identifier.clone(), builder.identifier("field").unwrap());
        let tuple = builder.tuple_expression(ExpressionList::single(literal.clone()));
        assert!(matches!(
            identifier.variant(),
            ExpressionVariant::Identifier(_)
        ));
        assert!(matches!(literal.variant(), ExpressionVariant::Literal(_)));
        assert!(matches!(call.variant(), ExpressionVariant::Call(_)));
        assert!(matches!(member.variant(), ExpressionVariant::Member(_)));
        assert!(matches!(tuple.variant(), ExpressionVariant::Tuple(_)));

        let (owned, _) = owned_fixture();
        let borrowed = owned
            .borrow_expression(false, identifier.clone())
            .expect("owned borrow expression");
        let capture = owned
            .closure_capture(ClosureCaptureMode::Value, name.clone(), None)
            .expect("owned closure capture");
        let closure = owned
            .closure_expression(
                vec![capture],
                ParameterList::empty(),
                owned.tuple_type(TypeSyntaxList::empty()),
                StatementList::empty(),
            )
            .expect("owned closure expression");
        assert!(matches!(
            borrowed.variant(),
            ExpressionVariant::Borrow { .. }
        ));
        assert!(matches!(closure.variant(), ExpressionVariant::Closure(_)));

        let wildcard = builder.wildcard_pattern();
        let binding = builder.binding_pattern(name.clone());
        let literal_pattern = builder.literal_pattern(LiteralValue::Bool(true));
        let tuple_pattern = builder.tuple_pattern(PatternList::single(wildcard.clone()));
        let variant_pattern = builder.variant_pattern(path.clone(), PatternList::empty());
        assert!(matches!(wildcard.variant(), PatternVariant::Wildcard));
        assert!(matches!(binding.variant(), PatternVariant::Binding(_)));
        assert!(matches!(
            literal_pattern.variant(),
            PatternVariant::Literal(_)
        ));
        assert!(matches!(tuple_pattern.variant(), PatternVariant::Tuple(_)));
        assert!(matches!(
            variant_pattern.variant(),
            PatternVariant::Variant { .. }
        ));

        let named = builder.named_type(path, TypeSyntaxList::empty());
        let tuple_type = builder.tuple_type(TypeSyntaxList::single(named.clone()));
        assert!(matches!(named.variant(), TypeSyntaxVariant::Named { .. }));
        assert!(matches!(tuple_type.variant(), TypeSyntaxVariant::Tuple(_)));
        let shared_reference = owned
            .reference_type(false, named.clone())
            .expect("owned reference type");
        let mutable_slice = owned
            .slice_type(true, named.clone())
            .expect("owned slice type");
        let array = owned
            .array_type(named.clone(), owned.literal(4_u64))
            .expect("owned array type");
        assert!(matches!(
            shared_reference.variant(),
            TypeSyntaxVariant::Reference { mutable: false, .. }
        ));
        assert!(matches!(
            mutable_slice.variant(),
            TypeSyntaxVariant::Slice { mutable: true, .. }
        ));
        assert!(matches!(array.variant(), TypeSyntaxVariant::Array { .. }));

        let expression_statement = builder.expression_statement(literal.clone());
        let return_statement = builder.return_statement(None);
        let let_statement = builder.let_statement(binding, Some(named), literal);
        assert!(matches!(
            expression_statement.variant(),
            StatementVariant::Expression(_)
        ));
        assert!(matches!(
            return_statement.variant(),
            StatementVariant::Return(None)
        ));
        assert!(matches!(
            let_statement.variant(),
            StatementVariant::Let { .. }
        ));
    }

    #[test]
    fn literal_builder_accepts_every_compile_time_primitive_form() {
        let (builder, _) = fixture();
        let values = [
            builder.literal(true),
            builder.literal('x'),
            builder.literal(-1_i64),
            builder.literal(1_u64),
            builder.literal(1.5_f64),
            builder.literal("text"),
            builder.literal(String::from("owned")),
            builder.literal(LiteralValue::Null),
        ];
        assert_eq!(values.len(), 8);
        assert!(
            values
                .iter()
                .all(|value| matches!(value.variant(), ExpressionVariant::Literal(_)))
        );
    }

    #[test]
    fn persistent_lists_and_with_transforms_leave_inputs_unchanged() {
        let (builder, _) = fixture();
        let first = builder.literal(LiteralValue::Unsigned(1));
        let second = builder.literal(LiteralValue::Unsigned(2));
        let left = ExpressionList::single(first);
        let right = ExpressionList::single(second);
        let combined = left.concat(&right);
        assert_eq!(left.length(), 1);
        assert_eq!(right.length(), 1);
        assert_eq!(combined.length(), 2);
        assert_eq!(combined.get(0).unwrap().display(), "1");
        assert_eq!(
            left.push(builder.literal(LiteralValue::Unsigned(3)))
                .length(),
            2
        );
        assert_eq!(combined.into_iter().count(), 2);

        let name = builder.identifier("value").unwrap();
        let field = builder.field(name, Visibility::Private, named_type(builder, "u64"));
        let public = field.with_visibility(Visibility::Public);
        assert_eq!(field.visibility(), Visibility::Private);
        assert_eq!(public.visibility(), Visibility::Public);
        assert_eq!(field.origin(), public.origin());
    }

    #[test]
    fn definitions_cover_items_members_metadata_and_structural_transforms() {
        let (builder, _) = fixture();
        let field = builder.field(
            builder.identifier("id").unwrap(),
            Visibility::Private,
            named_type(builder, "u64"),
        );
        let generic = builder.generic_parameter(
            builder.identifier("T").unwrap(),
            TypeSyntaxList::single(named_type(builder, "Display")),
        );
        let attribute_path = builder
            .path(vec![builder.identifier("example").unwrap()])
            .unwrap();
        let attribute = builder.attribute(attribute_path, ExpressionList::empty());
        let metadata = builder
            .metadata(builder.identifier("Entity").unwrap())
            .with_visibility(Visibility::Public)
            .with_documentation(vec![Arc::from("An entity.")])
            .with_generics(GenericParameterList::single(generic))
            .with_attributes(AttributeList::single(attribute));
        let definition =
            builder.struct_definition(metadata, MemberList::single(Member::Field(field)));
        let renamed = definition.with_metadata(
            definition
                .metadata()
                .with_name(builder.identifier("Record").unwrap()),
        );
        assert_eq!(definition.name().text(), "Entity");
        assert_eq!(renamed.name().text(), "Record");
        assert_eq!(renamed.metadata().visibility(), Visibility::Public);
        assert_eq!(renamed.metadata().documentation().len(), 1);
        assert_eq!(renamed.metadata().generics().length(), 1);
        assert_eq!(renamed.metadata().attributes().length(), 1);
        assert_eq!(renamed.fields().length(), 1);
        assert_eq!(renamed.members().length(), 1);
        assert_eq!(ItemList::single(Item::Struct(renamed)).length(), 1);
    }

    #[test]
    fn enum_function_and_implementation_values_are_detached_and_inspectable() {
        let (builder, _) = fixture();
        let unit = builder.tuple_type(TypeSyntaxList::empty());
        let parameter = builder.parameter(
            builder.identifier("value").unwrap(),
            named_type(builder, "u64"),
        );
        let function = builder.function(
            builder.metadata(builder.identifier("run").unwrap()),
            ParameterList::single(parameter),
            unit.clone(),
            StatementList::empty(),
        );
        let variant = builder.variant(builder.identifier("Ready").unwrap(), FieldList::empty());
        let enumeration = builder.enum_definition(
            builder.metadata(builder.identifier("State").unwrap()),
            VariantList::single(variant),
            MemberList::single(Member::Function(function.clone())),
        );
        let implementation = builder.implementation(
            named_type(builder, "Display"),
            named_type(builder, "State"),
            MemberList::single(Member::Function(function.clone())),
        );
        assert_eq!(enumeration.variants().length(), 1);
        assert_eq!(enumeration.type_syntax().display(), "State");
        assert_eq!(function.parameters().length(), 1);
        assert!(function.body().is_empty());
        assert_eq!(function.return_type(), &unit);
        assert_eq!(implementation.trait_type().display(), "Display");
        assert_eq!(implementation.target_type().display(), "State");
        assert_eq!(implementation.members().length(), 1);
    }

    #[test]
    fn every_structural_transform_preserves_the_original_value() {
        let (builder, _) = fixture();
        let first_name = builder.identifier("first").unwrap();
        let second_name = builder.identifier("second").unwrap();
        let first_type = named_type(builder, "u64");
        let second_type = named_type(builder, "String");

        let parameter = builder.parameter(first_name.clone(), first_type.clone());
        assert_eq!(
            parameter.with_name(second_name.clone()).name().text(),
            "second"
        );
        assert_eq!(
            parameter
                .with_type_syntax(second_type.clone())
                .type_syntax()
                .display(),
            "String"
        );
        assert_eq!(parameter.name().text(), "first");

        let generic = builder.generic_parameter(first_name.clone(), TypeSyntaxList::empty());
        assert_eq!(
            generic.with_name(second_name.clone()).name().text(),
            "second"
        );
        assert_eq!(
            generic
                .with_bounds(TypeSyntaxList::single(first_type.clone()))
                .bounds()
                .length(),
            1
        );

        let first_path = builder.path(vec![first_name.clone()]).unwrap();
        let second_path = builder.path(vec![second_name.clone()]).unwrap();
        let attribute = builder.attribute(first_path, ExpressionList::empty());
        assert_eq!(attribute.path().display(), "first");
        assert_eq!(attribute.with_path(second_path).path().display(), "second");
        assert_eq!(
            attribute
                .with_arguments(ExpressionList::single(builder.literal(true)))
                .arguments()
                .length(),
            1
        );

        let receiver = builder.identifier_expression(first_name);
        let member = builder.member(receiver, second_name.clone());
        let ExpressionVariant::Member(member) = member.variant() else {
            panic!("expected member expression")
        };
        assert_eq!(
            member
                .with_receiver(builder.literal(LiteralValue::Unsigned(1)))
                .receiver()
                .display(),
            "1"
        );
        assert_eq!(
            member
                .with_name(builder.identifier("third").unwrap())
                .name()
                .text(),
            "third"
        );

        let call = builder.call(
            builder.identifier_expression(builder.identifier("run").unwrap()),
            ExpressionList::empty(),
        );
        let ExpressionVariant::Call(call) = call.variant() else {
            panic!("expected call expression")
        };
        assert_eq!(
            call.with_callee(builder.literal(1_u64)).callee().display(),
            "1"
        );
        assert_eq!(
            call.with_arguments(ExpressionList::single(builder.literal(2_u64)))
                .arguments()
                .length(),
            1
        );

        let field = builder.field(second_name.clone(), Visibility::Private, first_type.clone());
        assert_eq!(
            field
                .with_name(builder.identifier("third").unwrap())
                .name()
                .text(),
            "third"
        );
        assert_eq!(
            field
                .with_type_syntax(second_type.clone())
                .type_syntax()
                .display(),
            "String"
        );

        let variant = builder.variant(second_name, FieldList::single(field.clone()));
        assert_eq!(
            variant
                .with_name(builder.identifier("Other").unwrap())
                .name()
                .text(),
            "Other"
        );
        assert!(variant.with_fields(FieldList::empty()).fields().is_empty());

        let function = builder.function(
            builder.metadata(builder.identifier("run").unwrap()),
            ParameterList::empty(),
            first_type.clone(),
            StatementList::empty(),
        );
        assert_eq!(
            function
                .with_name(builder.identifier("call").unwrap())
                .name()
                .text(),
            "call"
        );
        assert_eq!(
            function
                .with_visibility(Visibility::Public)
                .metadata()
                .visibility(),
            Visibility::Public
        );
        assert_eq!(
            function
                .with_parameters(ParameterList::single(parameter))
                .parameters()
                .length(),
            1
        );
        assert_eq!(
            function
                .with_return_type(second_type.clone())
                .return_type()
                .display(),
            "String"
        );
        assert_eq!(
            function
                .with_body(StatementList::single(builder.return_statement(None)))
                .body()
                .length(),
            1
        );

        let structure = builder.struct_definition(
            builder.metadata(builder.identifier("Record").unwrap()),
            MemberList::single(Member::Field(field)),
        );
        assert!(
            structure
                .with_fields(FieldList::empty())
                .fields()
                .is_empty()
        );
        assert!(
            structure
                .with_members(MemberList::empty())
                .members()
                .is_empty()
        );
        assert_eq!(
            structure
                .with_visibility(Visibility::Public)
                .metadata()
                .visibility(),
            Visibility::Public
        );

        let enumeration = builder.enum_definition(
            builder.metadata(builder.identifier("State").unwrap()),
            VariantList::single(variant),
            MemberList::empty(),
        );
        assert_eq!(
            enumeration
                .with_name(builder.identifier("Mode").unwrap())
                .name()
                .text(),
            "Mode"
        );
        assert!(
            enumeration
                .with_variants(VariantList::empty())
                .variants()
                .is_empty()
        );
        assert!(
            enumeration
                .with_members(MemberList::empty())
                .members()
                .is_empty()
        );
        assert_eq!(
            enumeration
                .with_visibility(Visibility::Public)
                .metadata()
                .visibility(),
            Visibility::Public
        );

        let implementation =
            builder.implementation(first_type.clone(), second_type.clone(), MemberList::empty());
        assert_eq!(
            implementation
                .with_trait_type(second_type.clone())
                .trait_type()
                .display(),
            "String"
        );
        assert_eq!(
            implementation
                .with_target_type(first_type)
                .target_type()
                .display(),
            "u64"
        );
        assert_eq!(
            implementation
                .with_members(MemberList::single(Member::Function(function)))
                .members()
                .length(),
            1
        );

        let inherent = builder.inherent_implementation(second_type, MemberList::empty());
        assert_eq!(inherent.target_type().display(), "String");
        assert!(inherent.members().is_empty());
    }

    #[test]
    fn std_ast_error_uses_physical_origins() {
        let (builder, provenance) = fixture();
        let node = builder.literal("bad");
        let error = AstInterface::current().error(&node, "bad generated syntax");
        let diagnostic = error.diagnostic(&provenance);
        assert_eq!(diagnostic.category, Category::CompileTime);
        assert_eq!(diagnostic.message, "bad generated syntax");
        assert!(diagnostic.primary.is_some());
    }

    #[test]
    fn generated_errors_retain_invocation_and_definition_context_without_a_fake_span() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(
            PathBuf::from("macro.elx"),
            "definition invocation".to_string(),
        );
        let mut provenance = ProvenanceTable::new();
        let definition = provenance.register_physical(Span::new(file, 0, 10));
        let invocation = provenance.register_physical(Span::new(file, 11, 21));
        let expansion = provenance.register_expansion(invocation, definition);
        let generated =
            provenance.register_generated(expansion, GeneratedSource::Definition(definition));
        let builder = AstInterface::current().builder(OriginHandle::from_range(OriginRange::new(
            generated, generated,
        )));
        let error = AstInterface::current().error(&builder.literal(false), "generated failure");
        let diagnostic = error.diagnostic(&provenance);

        assert_eq!(diagnostic.primary, None);
        assert_eq!(diagnostic.related.len(), 2);
        assert_eq!(diagnostic.related[0].1, "compile-time invocation");
        assert_eq!(diagnostic.related[1].1, "compile-time definition");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn persistent_list_concatenation_preserves_arbitrary_inputs(
            left_values in proptest::collection::vec(any::<u16>(), 0..48),
            right_values in proptest::collection::vec(any::<u16>(), 0..48),
        ) {
            let (builder, _) = fixture();
            let left = ExpressionList::from(
                left_values
                    .iter()
                    .map(|value| builder.literal(u64::from(*value)))
                    .collect::<Vec<_>>(),
            );
            let right = ExpressionList::from(
                right_values
                    .iter()
                    .map(|value| builder.literal(u64::from(*value)))
                    .collect::<Vec<_>>(),
            );
            let combined = left.concat(&right);

            prop_assert_eq!(left.length(), left_values.len());
            prop_assert_eq!(right.length(), right_values.len());
            prop_assert_eq!(combined.length(), left_values.len() + right_values.len());
            let actual = combined.iter().map(Expression::display).collect::<Vec<_>>();
            let expected = left_values
                .iter()
                .chain(&right_values)
                .map(u16::to_string)
                .collect::<Vec<_>>();
            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn every_noncurrent_interface_version_is_rejected(major in any::<u16>(), minor in any::<u16>()) {
            let required = AstInterfaceVersion { major, minor };
            prop_assume!(required != INTERFACE_VERSION);
            prop_assert_eq!(
                AstInterface::negotiate(required).unwrap_err(),
                AstError::VersionMismatch {
                    required,
                    provided: INTERFACE_VERSION,
                }
            );
        }
    }
}
