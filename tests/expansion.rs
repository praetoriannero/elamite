use std::path::{Path, PathBuf};

use elamite::expansion::{ExpandedUnitIdentity, expand};
use elamite::package::PackageGraph;
use elamite::parsed::{ParsedUnitIdentity, parse_package};
use elamite::resolution::{resolve, resolve_expanded};
use elamite::source::SourceManager;

fn spec_demo_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/spec_demo/elamite.toml")
}

fn graph(sources: &mut SourceManager) -> PackageGraph {
    PackageGraph::resolve(&spec_demo_manifest(), sources).expect("spec demo package graph resolves")
}

#[test]
fn expansion_owns_unit_identities_and_preserves_macro_free_inputs() {
    let mut sources = SourceManager::new();
    let graph = graph(&mut sources);
    let parsed = parse_package(&graph, &mut sources);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let expected = parsed
        .package
        .units
        .iter()
        .map(|unit| {
            (
                unit.identity.clone(),
                unit.path.clone(),
                unit.file,
                unit.span,
                unit.tokens.clone(),
                unit.tree.clone(),
            )
        })
        .collect::<Vec<_>>();
    let expanded = expand(parsed);
    assert!(expanded.diagnostics.is_empty());
    assert_eq!(expanded.package.units.len(), expected.len());

    for (unit, (identity, path, file, span, tokens, tree)) in
        expanded.package.units.iter().zip(expected)
    {
        let expected_identity = match identity {
            ParsedUnitIdentity::Standard(module) => ExpandedUnitIdentity::Standard(module),
            ParsedUnitIdentity::PackageRoot(package) => ExpandedUnitIdentity::PackageRoot(package),
            ParsedUnitIdentity::PackageModule { package, path } => {
                ExpandedUnitIdentity::PackageModule { package, path }
            }
        };
        assert_eq!(unit.identity, expected_identity);
        assert_eq!(unit.path, path);
        assert_eq!(unit.file, file);
        assert_eq!(unit.span, span);
        assert_eq!(unit.tokens, tokens);
        assert_eq!(unit.tree, tree);
        assert_eq!(unit.token_trees.flattened().len(), unit.tokens.len());
    }
}

#[test]
fn explicit_expansion_and_the_normal_resolver_entry_point_are_equivalent() {
    let mut direct_sources = SourceManager::new();
    let direct_graph = graph(&mut direct_sources);
    let direct = resolve(&direct_graph, &mut direct_sources);

    let mut explicit_sources = SourceManager::new();
    let explicit_graph = graph(&mut explicit_sources);
    let parsed = parse_package(&explicit_graph, &mut explicit_sources);
    let explicit = resolve_expanded(&explicit_graph, expand(parsed));

    assert_eq!(direct.diagnostics.len(), explicit.diagnostics.len());
    assert_eq!(direct.program.dump(), explicit.program.dump());
}
