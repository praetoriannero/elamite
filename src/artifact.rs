//! Deterministic public package metadata used by the native build workflow.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::docs;
use crate::package::{PackageGraph, PackageId};
use crate::resolution::{DeclarationId, ItemId, ResolvedProgram, Visibility};
use crate::source::SourceManager;
use crate::types::{ProvenanceSource, TypedProgram};

pub const FORMAT_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicApi {
    pub path: String,
    pub kind: String,
    pub signature: String,
    pub reexport: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_provenance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub format_version: u32,
    pub package_name: String,
    pub package_version: String,
    pub package_identity: String,
    pub semantic_revision: String,
    pub public_api: Vec<PublicApi>,
    pub include_paths: Vec<String>,
    pub library_paths: Vec<String>,
    pub native_libraries: Vec<String>,
    pub link_options: Vec<String>,
}

impl PackageMetadata {
    #[must_use]
    pub fn collect(
        graph: &PackageGraph,
        resolved: &ResolvedProgram,
        typed: &TypedProgram,
        sources: &SourceManager,
        package_id: &PackageId,
    ) -> Self {
        let package = &graph.packages[package_id];
        let mut public_api = docs::extract(resolved, sources, package_id)
            .items
            .into_iter()
            .map(|item| {
                let return_provenance = resolved
                    .declarations
                    .iter()
                    .find(|declaration| public_path(resolved, declaration.id) == item.path)
                    .and_then(|declaration| return_provenance(typed, declaration.id));
                PublicApi {
                    path: item.path,
                    kind: format!("{:?}", item.kind),
                    signature: item.signature,
                    reexport: false,
                    return_provenance,
                }
            })
            .collect::<Vec<_>>();

        for import in resolved.imports.iter().filter(|import| {
            import.visibility == Visibility::Public
                && resolved.modules[import.module.index()].package.as_ref() == Some(package_id)
        }) {
            let Some(ItemId::Declaration(target)) = import.target else {
                continue;
            };
            let declaration = &resolved.declarations[target.index()];
            let module = &resolved.modules[import.module.index()];
            let mut path = vec!["root".to_string()];
            path.extend(
                module
                    .path
                    .iter()
                    .map(|part| resolved.symbol_text(*part).to_string()),
            );
            path.push(resolved.symbol_text(import.name).to_string());
            public_api.push(PublicApi {
                path: path.join("."),
                kind: format!("{:?}", declaration.kind),
                signature: docs::declaration_signature(&declaration.syntax, sources),
                reexport: true,
                return_provenance: return_provenance(typed, target),
            });
        }
        public_api.sort_by(|left, right| {
            (&left.path, left.reexport, &left.signature).cmp(&(
                &right.path,
                right.reexport,
                &right.signature,
            ))
        });
        public_api.dedup();

        Self {
            format_version: FORMAT_VERSION,
            package_name: package.manifest.name.clone(),
            package_version: package.manifest.version.clone(),
            package_identity: package_id.display().to_string(),
            semantic_revision: package.semantic_revision().as_str().to_string(),
            public_api,
            include_paths: package
                .manifest
                .include_paths
                .iter()
                .map(|path| package.manifest_dir.join(path).display().to_string())
                .collect(),
            library_paths: package
                .manifest
                .library_paths
                .iter()
                .map(|path| package.manifest_dir.join(path).display().to_string())
                .collect(),
            native_libraries: package.manifest.native_libraries.clone(),
            link_options: package.manifest.link_options.clone(),
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        let encoded = toml::to_string(self)
            .map_err(|error| format!("cannot encode package metadata: {error}"))?;
        std::fs::write(path, encoded)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let encoded = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let metadata: Self = toml::from_str(&encoded)
            .map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
        if metadata.format_version != FORMAT_VERSION {
            return Err(format!(
                "{} uses metadata format {}, expected {}",
                path.display(),
                metadata.format_version,
                FORMAT_VERSION
            ));
        }
        Ok(metadata)
    }
}

fn public_path(resolved: &ResolvedProgram, declaration: DeclarationId) -> String {
    let declaration = &resolved.declarations[declaration.index()];
    let module = &resolved.modules[declaration.module.index()];
    let mut components = vec!["root".to_string()];
    components.extend(
        module
            .path
            .iter()
            .map(|part| resolved.symbol_text(*part).to_string()),
    );
    if let Some(parent) = declaration.parent_declaration {
        components.push(
            resolved
                .symbol_text(resolved.declarations[parent.index()].name)
                .to_string(),
        );
    }
    components.push(resolved.symbol_text(declaration.name).to_string());
    components.join(".")
}

fn return_provenance(typed: &TypedProgram, declaration: DeclarationId) -> Option<String> {
    typed
        .function_provenance
        .get(&declaration)
        .map(|provenance| match provenance.returned_from {
            ProvenanceSource::Static => "static".to_string(),
            ProvenanceSource::Receiver => "receiver".to_string(),
            ProvenanceSource::Parameter(index) => format!("parameter:{index}"),
            ProvenanceSource::ClosureCapture(index) => format!("capture:{index}"),
            ProvenanceSource::AllBorrowingInputs => "all-borrowing-inputs".to_string(),
        })
}
