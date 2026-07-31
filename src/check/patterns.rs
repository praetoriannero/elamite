//! Pattern typing, reachability, and exhaustiveness analysis.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn check_match_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        let children = child_nodes(node);
        let scrutinee_type = children
            .first()
            .map(|scrutinee| self.check_expr(scrutinee, ExpectedType::None).0)
            .unwrap_or_else(|| self.typed.types.error());
        if self.is_never_type(scrutinee_type) {
            return true;
        }
        let Some(block) = children
            .into_iter()
            .find(|child| child.kind == SyntaxKind::Block)
        else {
            return false;
        };
        let mut coverage = Coverage::None;
        let mut all_arms_return = true;
        let mut any_arm = false;
        for arm in child_nodes(block) {
            if arm.kind != SyntaxKind::MatchArm {
                continue;
            }
            any_arm = true;
            let unreachable = coverage.is_catchall();
            if unreachable {
                let span = child_nodes(arm).first().map_or(arm.span, |p| p.span);
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this match arm is unreachable; an earlier arm already matches every value",
                    )
                    .with_primary(span),
                );
            }
            let mut guarded = false;
            let mut arm_coverage = Coverage::None;
            let mut arm_returns = false;
            for child in child_nodes(arm) {
                match child.kind {
                    SyntaxKind::Pattern | SyntaxKind::AlternativePattern => {
                        arm_coverage = self.check_pattern(child, scrutinee_type);
                        if !unreachable && !guarded {
                            let redundant = arm_coverage.is_covered_by(&coverage);
                            if redundant {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::Pattern,
                                        "this match arm is unreachable; its pattern is already \
                                         fully covered by an earlier arm",
                                    )
                                    .with_primary(child.span),
                                );
                            }
                        }
                    }
                    SyntaxKind::Guard => {
                        guarded = true;
                        if let Some(condition) = child_nodes(child).into_iter().next() {
                            self.check_condition(condition);
                        }
                    }
                    SyntaxKind::Block => arm_returns = self.check_block(child, return_type),
                    _ => {}
                }
            }
            // Guarded arms do not contribute to exhaustiveness or
            // redundancy tracking (`SPEC.md` section 7): the guard may fail
            // at runtime, so this arm cannot be assumed to consume coverage.
            if !guarded {
                coverage = coverage.union(arm_coverage);
            }
            if !arm_returns {
                all_arms_return = false;
            }
        }
        let exhaustive = self.coverage_is_exhaustive(scrutinee_type, &coverage);
        if any_arm && !exhaustive {
            let missing = self.describe_missing_coverage(scrutinee_type, &coverage);
            if let Some(missing) = missing {
                self.diagnostics
                    .push(Diagnostic::new(Category::Pattern, missing).with_primary(node.span));
            }
        }
        any_arm && exhaustive && all_arms_return
    }

    /// Whether `coverage` matches every value of `scrutinee_type`: literally
    /// `Coverage::Catchall`, or `Coverage::Bools`/`Coverage::Variants`
    /// reaching the scrutinee's full closed domain.
    pub(super) fn coverage_is_exhaustive(
        &self,
        scrutinee_type: TypeId,
        coverage: &Coverage,
    ) -> bool {
        match coverage {
            Coverage::Catchall => true,
            Coverage::Dereferenced(inner) => {
                let mut resolved = self.typed.types.resolve_inference(scrutinee_type);
                loop {
                    match self.typed.types.kind(resolved) {
                        TypeKind::Alias { target, .. } => resolved = *target,
                        TypeKind::Reference { target, .. } => {
                            return self.coverage_is_exhaustive(*target, inner);
                        }
                        _ => return false,
                    }
                }
            }
            Coverage::Bools(values) => values.len() >= 2,
            Coverage::Variants(variants) => {
                let resolved = self.typed.types.resolve_inference(scrutinee_type);
                match self.typed.types.kind(resolved) {
                    TypeKind::Nominal { identity, .. } => self
                        .resolved
                        .variants
                        .iter()
                        .filter(|variant| variant.parent == identity.declaration)
                        .all(|variant| variants.contains(&variant.id)),
                    _ => false,
                }
            }
            Coverage::None | Coverage::Other => false,
        }
    }

    /// Produces a diagnostic message for a non-exhaustive match, or `None`
    /// when the scrutinee's shape is not one Milestone 7 reasons about
    /// precisely (tuples/structs with a refutable field, and any other
    /// non-`bool`/non-`enum` type): those conservatively require an
    /// explicit catch-all arm, matching `SPEC.md`'s "infinite domain" rule,
    /// without this module claiming to enumerate their exact cases.
    pub(super) fn describe_missing_coverage(
        &self,
        scrutinee_type: TypeId,
        coverage: &Coverage,
    ) -> Option<String> {
        let resolved = self.typed.types.resolve_inference(scrutinee_type);
        if let Coverage::Dereferenced(inner) = coverage {
            return match self.typed.types.kind(resolved) {
                TypeKind::Reference { target, .. } => {
                    self.describe_missing_coverage(*target, inner)
                }
                _ => None,
            };
        }
        match self.typed.types.kind(resolved) {
            TypeKind::Primitive(PrimitiveType::Bool) => Some(
                "this match is not exhaustive; cover both `true` and `false`, or add a \
                 catch-all `_` arm"
                    .to_string(),
            ),
            TypeKind::Nominal { identity, .. }
                if self.resolved.declarations[identity.declaration.index()].kind
                    == DeclarationKind::Enum =>
            {
                let covered = match coverage {
                    Coverage::Variants(variants) => variants.clone(),
                    _ => BTreeSet::new(),
                };
                let missing: Vec<&str> = self
                    .resolved
                    .variants
                    .iter()
                    .filter(|variant| variant.parent == identity.declaration)
                    .filter(|variant| !covered.contains(&variant.id))
                    .map(|variant| self.resolved.symbol_text(variant.name))
                    .collect();
                if missing.is_empty() {
                    None
                } else {
                    Some(format!(
                        "this match is not exhaustive; unmatched variant(s): {}",
                        missing.join(", ")
                    ))
                }
            }
            TypeKind::Error => None,
            _ => Some("this match is not exhaustive; add a catch-all `_` arm".to_string()),
        }
    }

    // ---------------------------------------------------------------
    // Patterns
    // ---------------------------------------------------------------

    /// Type-checks `pattern` against `scrutinee_type`, registers its
    /// bindings (as ordinary immutable, independently copied `let`-like
    /// bindings per `SPEC.md` section 7) into the same local-type tables
    /// `check_expr` reads, and returns what the pattern covers for
    /// exhaustiveness/reachability purposes.
    ///
    /// This module reasons precisely about coverage for `bool` and `enum`
    /// scrutinees and for any pattern that is unconditionally irrefutable
    /// (`_`, a plain binding, or a tuple/struct pattern built entirely from
    /// those). Nested field-value refutability inside a matched enum
    /// variant is not tracked (an arm's outer constructor is treated as
    /// fully covering that variant once matched); everything else
    /// (tuples/structs with a refutable field, literals of an unbounded
    /// domain) conservatively requires an explicit catch-all, matching
    /// `SPEC.md`'s "infinite domain" rule rather than risking a false
    /// exhaustiveness claim.
    pub(super) fn check_pattern(
        &mut self,
        pattern: &SyntaxNode,
        scrutinee_type: TypeId,
    ) -> Coverage {
        match pattern.kind {
            SyntaxKind::AlternativePattern => {
                let atoms = child_nodes(pattern);
                let mut binding_sets = Vec::with_capacity(atoms.len());
                let mut coverage = Coverage::None;
                for atom in &atoms {
                    let mut names = BTreeSet::new();
                    collect_pattern_binding_names(atom, &mut names);
                    binding_sets.push(names);
                    coverage = coverage.union(self.check_pattern(atom, scrutinee_type));
                }
                if let Some(first_names) = binding_sets.first().cloned() {
                    for (atom, names) in atoms.iter().zip(binding_sets.iter()).skip(1) {
                        if *names != first_names {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Pattern,
                                    "every alternative in a `|` pattern must bind the same names",
                                )
                                .with_primary(atom.span),
                            );
                        }
                    }
                }
                coverage
            }
            SyntaxKind::TuplePattern => {
                let elements = child_nodes(pattern);
                let resolved = self.typed.types.resolve_inference(scrutinee_type);
                let members = match self.typed.types.kind(resolved) {
                    TypeKind::Tuple(members) if members.len() == elements.len() => {
                        Some(members.clone())
                    }
                    _ => None,
                };
                if members.is_none() && scrutinee_type != self.typed.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Pattern,
                            "this tuple pattern does not match the scrutinee's type",
                        )
                        .with_primary(pattern.span),
                    );
                }
                let mut all_irrefutable = true;
                for (index, element) in elements.iter().enumerate() {
                    let element_type = members
                        .as_ref()
                        .map_or_else(|| self.typed.types.error(), |members| members[index]);
                    if !self.check_pattern(element, element_type).is_catchall() {
                        all_irrefutable = false;
                    }
                }
                if all_irrefutable {
                    Coverage::Catchall
                } else {
                    Coverage::Other
                }
            }
            SyntaxKind::DereferencePattern => {
                let Some(inner) = child_nodes(pattern).into_iter().next() else {
                    return Coverage::Other;
                };
                let mut resolved = self.typed.types.resolve_inference(scrutinee_type);
                loop {
                    match self.typed.types.kind(resolved) {
                        TypeKind::Alias { target, .. } => resolved = *target,
                        TypeKind::Reference { target, .. } => {
                            return Coverage::Dereferenced(Box::new(
                                self.check_pattern(inner, *target),
                            ));
                        }
                        TypeKind::Error => return self.check_pattern(inner, resolved),
                        _ => {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Pattern,
                                    "a dereference pattern requires a safe-reference scrutinee",
                                )
                                .with_primary(pattern.span),
                            );
                            return self.check_pattern(inner, self.typed.types.error());
                        }
                    }
                }
            }
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                self.check_aggregate_pattern(pattern, scrutinee_type)
            }
            SyntaxKind::Pattern => {
                let nested = child_nodes(pattern);
                if let Some(&atom) = nested.first() {
                    return self.check_pattern(atom, scrutinee_type);
                }
                self.check_pattern_leaf(pattern, scrutinee_type)
            }
            _ => Coverage::Other,
        }
    }

    pub(super) fn check_pattern_leaf(
        &mut self,
        pattern: &SyntaxNode,
        scrutinee_type: TypeId,
    ) -> Coverage {
        if let Some(token) = pattern.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) if is_pattern_literal_token(&token.kind) => Some(token),
            _ => None,
        }) {
            let ty = self.literal_token_type(token, ExpectedType::Exact(scrutinee_type), false);
            if !self.types_compatible(ty, scrutinee_type) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            return match token.kind {
                TokenKind::Keyword(Keyword::True) => Coverage::Bools(BTreeSet::from([true])),
                TokenKind::Keyword(Keyword::False) => Coverage::Bools(BTreeSet::from([false])),
                _ => Coverage::Other,
            };
        }
        let identifiers: Vec<&Token> = pattern
            .children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                    Some(token)
                }
                _ => None,
            })
            .collect();
        let has_dot = pattern.children.iter().any(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Dot,
                    ..
                })
            )
        });
        if has_dot {
            let (Some(&first), Some(&last)) = (identifiers.first(), identifiers.last()) else {
                return Coverage::Other;
            };
            let Some((enum_declaration, variant)) = self.resolve_pattern_variant(first, last)
            else {
                return Coverage::Other;
            };
            if self
                .nominal_arguments_for_type(scrutinee_type, enum_declaration)
                .is_none()
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            if !self.resolved.variants[variant.index()].fields.is_empty() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this variant has fields and must be matched with `(...)` or `{...}`",
                    )
                    .with_primary(pattern.span),
                );
            }
            return Coverage::Variants(BTreeSet::from([variant]));
        }
        if let [token] = identifiers.as_slice() {
            let text = token_text(token);
            if text != "_" {
                self.bind_pattern(token, scrutinee_type);
            }
        }
        Coverage::Catchall
    }

    pub(super) fn resolve_pattern_variant(
        &self,
        first: &Token,
        last: &Token,
    ) -> Option<(DeclarationId, VariantId)> {
        let target = self.resolved.reference_at(first.span)?.target;
        let NameTarget::Item(crate::resolution::ItemId::Declaration(enum_declaration)) = target
        else {
            return None;
        };
        if self.resolved.declarations[enum_declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        match self.find_member(enum_declaration, &token_text(last)) {
            Some(MemberId::Variant(variant)) => Some((enum_declaration, variant)),
            _ => None,
        }
    }

    pub(super) fn nominal_arguments_for_type(
        &self,
        mut ty: TypeId,
        declaration: DeclarationId,
    ) -> Option<Vec<TypeId>> {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                TypeKind::Nominal {
                    identity,
                    arguments,
                } if identity.declaration == declaration => return Some(arguments.clone()),
                TypeKind::Error => return Some(Vec::new()),
                _ => return None,
            }
        }
    }

    pub(super) fn check_aggregate_pattern(
        &mut self,
        pattern: &SyntaxNode,
        scrutinee_type: TypeId,
    ) -> Coverage {
        let path_tokens = leading_pattern_path(pattern);
        let Some(&first) = path_tokens.first() else {
            return Coverage::Other;
        };
        let has_dot = path_tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Dot));
        if has_dot {
            let Some(&last) = path_tokens
                .iter()
                .rev()
                .find(|token| matches!(token.kind, TokenKind::Identifier(_)))
            else {
                return Coverage::Other;
            };
            let Some((enum_declaration, variant)) = self.resolve_pattern_variant(first, last)
            else {
                return Coverage::Other;
            };
            let owner_arguments = self.nominal_arguments_for_type(scrutinee_type, enum_declaration);
            if owner_arguments.is_none() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            let fields = self.resolved.variants[variant.index()].fields.clone();
            let owner_arguments = owner_arguments.unwrap_or_default();
            match pattern.kind {
                SyntaxKind::VariantPattern => self.check_variant_positional_pattern(
                    pattern,
                    &fields,
                    enum_declaration,
                    &owner_arguments,
                ),
                SyntaxKind::RecordPattern => {
                    self.check_record_pattern_fields(
                        pattern,
                        &fields,
                        enum_declaration,
                        &owner_arguments,
                    );
                }
                _ => {}
            }
            return Coverage::Variants(BTreeSet::from([variant]));
        }
        let Some(reference) = self.resolved.reference_at(first.span) else {
            return Coverage::Other;
        };
        let NameTarget::Item(crate::resolution::ItemId::Declaration(declaration_id)) =
            reference.target
        else {
            return Coverage::Other;
        };
        let declaration = &self.resolved.declarations[declaration_id.index()];
        if declaration.kind != DeclarationKind::Struct {
            return Coverage::Other;
        }
        let owner_arguments = self.nominal_arguments_for_type(scrutinee_type, declaration_id);
        if owner_arguments.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    "this pattern's type does not match the scrutinee's type",
                )
                .with_primary(pattern.span),
            );
        }
        let fields: Vec<FieldId> = self
            .resolved
            .fields
            .iter()
            .filter(|field| {
                field.parent_declaration == declaration_id && field.parent_variant.is_none()
            })
            .map(|field| field.id)
            .collect();
        let all_irrefutable = match pattern.kind {
            SyntaxKind::RecordPattern => self.check_record_pattern_fields(
                pattern,
                &fields,
                declaration_id,
                &owner_arguments.unwrap_or_default(),
            ),
            _ => false,
        };
        if all_irrefutable {
            Coverage::Catchall
        } else {
            Coverage::Other
        }
    }

    /// Checks a `VariantPattern`'s parenthesized positional sub-patterns
    /// against a tuple-like variant's field types.
    pub(super) fn check_variant_positional_pattern(
        &mut self,
        pattern: &SyntaxNode,
        fields: &[FieldId],
        _owner: DeclarationId,
        owner_arguments: &[TypeId],
    ) {
        let sub_patterns = child_nodes(pattern);
        if sub_patterns.len() != fields.len() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    format!(
                        "this variant has {} field{}, but the pattern supplies {}",
                        fields.len(),
                        if fields.len() == 1 { "" } else { "s" },
                        sub_patterns.len(),
                    ),
                )
                .with_primary(pattern.span),
            );
        }
        for (index, sub_pattern) in sub_patterns.iter().enumerate() {
            let field_type = fields
                .get(index)
                .and_then(|field| {
                    self.typed
                        .instantiate_field_type(self.resolved, *field, owner_arguments)
                })
                .unwrap_or_else(|| self.typed.types.error());
            self.check_pattern(sub_pattern, field_type);
        }
    }

    /// Checks a `RecordPattern`'s named fields (struct or record-variant)
    /// and returns whether the pattern is irrefutable: every explicit field
    /// sub-pattern is itself irrefutable, or `..` was used to ignore the
    /// rest. Enforces "every field must appear, or use `..`" (`SPEC.md`
    /// section 7).
    pub(super) fn check_record_pattern_fields(
        &mut self,
        pattern: &SyntaxNode,
        required: &[FieldId],
        _owner: DeclarationId,
        owner_arguments: &[TypeId],
    ) -> bool {
        let mut seen: BTreeSet<FieldId> = BTreeSet::new();
        let mut has_rest = false;
        let mut all_irrefutable = true;
        for field_node in child_nodes(pattern) {
            if field_node.kind != SyntaxKind::PatternField {
                continue;
            }
            if matches!(
                field_node.children.first(),
                Some(SyntaxElement::Token(Token {
                    kind: TokenKind::DotDot,
                    ..
                }))
            ) {
                has_rest = true;
                continue;
            }
            let Some(name_token) = first_identifier_token(field_node) else {
                continue;
            };
            let name_text = token_text(name_token);
            let matched = required.iter().copied().find(|field_id| {
                self.resolved
                    .symbol_text(self.resolved.fields[field_id.index()].name)
                    == name_text
            });
            let Some(field_id) = matched else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        format!("no field named `{name_text}` on this pattern's type"),
                    )
                    .with_primary(name_token.span),
                );
                all_irrefutable = false;
                continue;
            };
            self.require_field_access(field_id, name_token.span);
            if !seen.insert(field_id) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        format!("field `{name_text}` is matched more than once"),
                    )
                    .with_primary(name_token.span),
                );
            }
            let field_type = self
                .typed
                .instantiate_field_type(self.resolved, field_id, owner_arguments)
                .unwrap_or_else(|| self.typed.types.error());
            match child_nodes(field_node).into_iter().next() {
                Some(sub_pattern) => {
                    if !self.check_pattern(sub_pattern, field_type).is_catchall() {
                        all_irrefutable = false;
                    }
                }
                None => {
                    if name_text != "_" {
                        self.bind_pattern(name_token, field_type);
                    }
                }
            }
        }
        if !has_rest {
            for field_id in required {
                if !seen.contains(field_id) {
                    if !self.require_field_access(*field_id, pattern.span) {
                        all_irrefutable = false;
                        continue;
                    }
                    let name = self
                        .resolved
                        .symbol_text(self.resolved.fields[field_id.index()].name);
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Pattern,
                            format!("this pattern is missing field `{name}`; add it or use `..`"),
                        )
                        .with_primary(pattern.span),
                    );
                    all_irrefutable = false;
                }
            }
        } else {
            // Fields ignored by `..` are untested, so they cannot make the
            // pattern refutable.
        }
        has_rest || all_irrefutable
    }

    pub(super) fn bind_pattern(&mut self, token: &Token, ty: TypeId) {
        if let Some(&id) = self.span_to_local.get(&token.span) {
            self.local_types.insert(id, ty);
            self.local_rebindable.insert(id, Rebindable::Let);
            self.program.copied_pattern_bindings.insert(id);
            self.program.pattern_binding_types.insert(id, ty);
        }
    }

    // ---------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------
}
