//! Stable dependency identities for compile-time artifacts and expanded input.

use crate::config::SemanticRevision;
use crate::syntax::{SyntaxElement, SyntaxNode, TokenKind};

use super::ExpandedUnit;
use super::ast::AstInterfaceVersion;
use super::namespace::CompileTimeEnvironment;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactIdentity(pub u64);

impl std::fmt::Display for ArtifactIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpansionIdentities {
    pub package: ArtifactIdentity,
    pub declarations: Vec<ArtifactIdentity>,
    pub interface: ArtifactIdentity,
}

#[must_use]
pub fn calculate(
    semantic_revision: SemanticRevision,
    interface_version: AstInterfaceVersion,
    units: &[ExpandedUnit],
    environment: &CompileTimeEnvironment,
) -> ExpansionIdentities {
    let interface = hash_bytes(
        format!(
            "elamite:{};spec:{};ast:{}.{}",
            crate::version(),
            semantic_revision.as_str(),
            interface_version.major,
            interface_version.minor
        )
        .as_bytes(),
    );
    let declarations = environment
        .declarations
        .iter()
        .map(|declaration| {
            let mut hash = StableHasher::new();
            hash.bytes(&interface.0.to_le_bytes());
            hash.bytes(format!("{:?}", declaration.namespace).as_bytes());
            hash.bytes(declaration.name.as_bytes());
            hash.syntax(&declaration.syntax);
            ArtifactIdentity(hash.finish())
        })
        .collect::<Vec<_>>();
    let mut package = StableHasher::new();
    package.bytes(&interface.0.to_le_bytes());
    for unit in units {
        package.bytes(
            match &unit.identity {
                super::ExpandedUnitIdentity::Standard(module) => format!("std:{module:?}"),
                super::ExpandedUnitIdentity::PackageRoot(_) => "package:root".to_string(),
                super::ExpandedUnitIdentity::PackageModule { path, .. } => {
                    format!("package:{}", path.display())
                }
            }
            .as_bytes(),
        );
        package.syntax(&unit.tree);
    }
    for identity in &declarations {
        package.bytes(&identity.0.to_le_bytes());
    }
    for import in &environment.imports {
        package.bytes(
            format!("{:?}:{}:{:?}", import.namespace, import.name, import.target).as_bytes(),
        );
    }
    ExpansionIdentities {
        package: ArtifactIdentity(package.finish()),
        declarations,
        interface,
    }
}

#[must_use]
pub fn hash_tokens(tokens: &[crate::syntax::Token]) -> ArtifactIdentity {
    let mut hash = StableHasher::new();
    for token in tokens {
        hash.token(&token.kind);
    }
    ArtifactIdentity(hash.finish())
}

fn hash_bytes(bytes: &[u8]) -> ArtifactIdentity {
    let mut hash = StableHasher::new();
    hash.bytes(bytes);
    ArtifactIdentity(hash.finish())
}

struct StableHasher(u64);

impl StableHasher {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    fn token(&mut self, kind: &TokenKind) {
        self.bytes(format!("{kind:?}").as_bytes());
    }

    fn syntax(&mut self, node: &SyntaxNode) {
        self.bytes(format!("{:?}", node.kind).as_bytes());
        for child in &node.children {
            match child {
                SyntaxElement::Token(token) => self.token(&token.kind),
                SyntaxElement::Node(node) => self.syntax(node),
            }
        }
    }

    const fn finish(self) -> u64 {
        self.0
    }
}
