use std::path::PathBuf;

use elamite::expansion::fragment::parse_fragment;
use elamite::expansion::provenance::{GeneratedSource, Origin, ProvenanceTable};
use elamite::expansion::token_tree::{TokenTreeToken, build_token_trees};
use elamite::lexer::lex;
use elamite::parser::FragmentKind;
use elamite::source::{SourceManager, Span};
use elamite::syntax::TokenKind;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_lexer_output_stays_lossless_and_fragment_recovery_never_panics(
        characters in proptest::collection::vec(any::<char>(), 0..192),
    ) {
        let source: String = characters.into_iter().collect();
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("generated-expansion.elx"), source.clone());
        let lexed = lex(file, &source);
        let mut provenance = ProvenanceTable::new();
        let unit = build_token_trees(
            Span::new(file, 0, u32::try_from(source.len()).unwrap_or(u32::MAX)),
            source.clone(),
            &lexed.tokens,
            &mut provenance,
        );

        let flattened = unit.flattened();
        prop_assert_eq!(flattened.len(), lexed.tokens.len());
        for (represented, original) in flattened.iter().zip(&lexed.tokens) {
            prop_assert_eq!(&represented.kind, &original.kind);
            prop_assert_eq!(provenance.physical_span(represented.origin), Some(original.span));
            prop_assert_eq!(
                represented.source.as_str(),
                source
                    .get(original.span.start as usize..original.span.end as usize)
                    .unwrap_or_default()
            );
            prop_assert!(
                matches!(provenance.origin(represented.origin), Origin::Physical { .. }),
                "token-tree origin must remain physical",
            );
        }

        let boundary = unit.eof.as_ref().expect("the lexer always emits EOF").origin;
        for kind in [
            FragmentKind::Expression,
            FragmentKind::Statement,
            FragmentKind::Pattern,
            FragmentKind::Type,
            FragmentKind::Item,
        ] {
            let parsed = parse_fragment(&unit.trees, boundary, kind, &provenance)
                .expect("physical origins are always adaptable to parser spans");
            for diagnostic in parsed.diagnostics {
                if let Some(span) = diagnostic.primary {
                    prop_assert_eq!(span.file, file);
                    prop_assert!(span.start <= span.end);
                    prop_assert!(span.end as usize <= source.len());
                }
            }
        }
    }

    #[test]
    fn generated_provenance_chains_remain_acyclic_and_spanless(depth in 1usize..48) {
        let mut sources = SourceManager::new();
        let file = sources.add_text(
            PathBuf::from("provenance.elx"),
            "definition invocation".to_string(),
        );
        let mut provenance = ProvenanceTable::new();
        let definition = provenance.register_physical(Span::new(file, 0, 10));
        let mut invocation = provenance.register_physical(Span::new(file, 11, 21));
        let mut final_origin = invocation;

        for _ in 0..depth {
            let expansion = provenance.register_expansion(invocation, definition);
            let generated = TokenTreeToken::generated(
                TokenKind::Identifier("nested".to_string()),
                "nested".to_string(),
                expansion,
                GeneratedSource::Definition(definition),
                &mut provenance,
            );
            prop_assert_eq!(provenance.physical_span(generated.origin), None);
            invocation = generated.origin;
            final_origin = generated.origin;
        }

        let trace = provenance.trace(final_origin);
        prop_assert_eq!(trace.expansions.len(), depth);
        prop_assert_eq!(trace.source_chain.last(), Some(&definition));
        let mut expansion_ids = trace
            .expansions
            .iter()
            .map(|frame| frame.expansion)
            .collect::<Vec<_>>();
        expansion_ids.sort();
        expansion_ids.dedup();
        prop_assert_eq!(expansion_ids.len(), depth);
    }
}
