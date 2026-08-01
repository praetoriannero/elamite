//! Declaration-signature, body, lexical-binding, and expression resolution.

use super::*;

impl<'a> Resolver<'a> {
    pub(super) fn resolve_all_declaration_contents(&mut self) {
        for index in 0..self.program.impls.len() {
            let implementation = ImplId(index as u32);
            let syntax = self.program.impls[index].syntax.clone();
            let module = self.program.impls[index].module;
            let generics = self.generic_scope_for_impl(implementation);
            self.resolve_non_body_syntax(&syntax, module, &generics, false, true);
        }
        for index in 0..self.program.declarations.len() {
            self.resolve_declaration(DeclarationId(index as u32));
        }
    }

    pub(super) fn resolve_declaration(&mut self, declaration: DeclarationId) {
        let item = &self.program.declarations[declaration.index()];
        let syntax = item.syntax.clone();
        let module = item.module;
        let kind = item.kind;
        let generics = self.generic_scope_for_declaration(declaration);
        let self_allowed = item.parent_declaration.is_some()
            || item.parent_impl.is_some()
            || matches!(kind, DeclarationKind::Struct | DeclarationKind::Trait);
        self.resolve_non_body_syntax(&syntax, module, &generics, self_allowed, false);
        if matches!(
            kind,
            DeclarationKind::Struct | DeclarationKind::ForeignStruct
        ) {
            let fields = self
                .program
                .fields
                .iter()
                .filter(|field| {
                    field.parent_declaration == declaration && field.parent_variant.is_none()
                })
                .map(|field| field.syntax.clone())
                .collect::<Vec<_>>();
            for field in fields {
                self.resolve_non_body_syntax(&field, module, &generics, true, false);
            }
        }
        if kind == DeclarationKind::Enum {
            let variants = self
                .program
                .variants
                .iter()
                .filter(|variant| variant.parent == declaration)
                .map(|variant| variant.syntax.clone())
                .collect::<Vec<_>>();
            for variant in variants {
                self.resolve_non_body_syntax(&variant, module, &generics, false, false);
            }
        }
        if kind == DeclarationKind::Function
            || (kind == DeclarationKind::Test
                && self.resolve_test_bodies
                && self.program.modules[module.index()].package.as_ref() == Some(&self.graph.root))
        {
            self.resolve_function_body(declaration, &syntax, module, &generics, self_allowed);
        }
    }

    pub(super) fn generic_scope_for_impl(
        &self,
        implementation: ImplId,
    ) -> BTreeMap<Symbol, GenericParameterId> {
        self.program.impls[implementation.index()]
            .generic_parameters
            .iter()
            .map(|parameter| {
                let parameter = &self.program.generic_parameters[parameter.index()];
                (parameter.name, parameter.id)
            })
            .collect()
    }

    pub(super) fn generic_scope_for_declaration(
        &self,
        declaration: DeclarationId,
    ) -> BTreeMap<Symbol, GenericParameterId> {
        let mut chain = Vec::new();
        let mut current = Some(declaration);
        while let Some(id) = current {
            chain.push(id);
            current = self.program.declarations[id.index()].parent_declaration;
        }
        chain.reverse();
        let mut scope = BTreeMap::new();
        if let Some(implementation) = self.program.declarations[declaration.index()].parent_impl {
            for parameter in &self.program.impls[implementation.index()].generic_parameters {
                let parameter = &self.program.generic_parameters[parameter.index()];
                scope.insert(parameter.name, parameter.id);
            }
        }
        for id in chain {
            for parameter in &self.program.declarations[id.index()].generic_parameters {
                let parameter = &self.program.generic_parameters[parameter.index()];
                scope.insert(parameter.name, parameter.id);
            }
        }
        scope
    }

    pub(super) fn resolve_non_body_syntax(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        implementation_root: bool,
    ) {
        for child in &node.children {
            let SyntaxElement::Node(child) = child else {
                continue;
            };
            if child.kind == SyntaxKind::Block {
                continue;
            }
            if child.kind == SyntaxKind::Function && node.kind != SyntaxKind::Function {
                continue;
            }
            match child.kind {
                SyntaxKind::Type => {
                    self.resolve_type(child, module, generics, self_allowed);
                }
                SyntaxKind::DeriveList => {
                    self.resolve_derive_list(child, module, generics, self_allowed);
                }
                SyntaxKind::GenericParameter | SyntaxKind::GenericParameters => {
                    self.resolve_non_body_syntax(
                        child,
                        module,
                        generics,
                        self_allowed,
                        implementation_root,
                    );
                }
                _ => self.resolve_non_body_syntax(
                    child,
                    module,
                    generics,
                    self_allowed,
                    implementation_root,
                ),
            }
        }
        if implementation_root && node.kind == SyntaxKind::Impl {
            // The two direct `Type` children above are the trait and target;
            // method blocks are deliberately handled as declarations.
        }
    }

    pub(super) fn resolve_derive_list(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let mut current = Vec::new();
        for child in &node.children {
            match child {
                SyntaxElement::Token(Token {
                    kind: TokenKind::Comma,
                    ..
                }) => {
                    self.resolve_token_path(module, &current, generics, self_allowed, node.span);
                    current.clear();
                }
                SyntaxElement::Token(token)
                    if matches!(
                        token.kind,
                        TokenKind::Identifier(_)
                            | TokenKind::Keyword(
                                Keyword::Root
                                    | Keyword::SelfValue
                                    | Keyword::SelfType
                                    | Keyword::Super
                            )
                            | TokenKind::Dot
                    ) =>
                {
                    current.push(token.clone());
                }
                _ => {}
            }
        }
        self.resolve_token_path(module, &current, generics, self_allowed, node.span);
    }

    pub(super) fn resolve_type(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let direct_tokens = node
            .children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Token(token)
                    if matches!(
                        token.kind,
                        TokenKind::Identifier(_)
                            | TokenKind::Keyword(
                                Keyword::Root
                                    | Keyword::SelfValue
                                    | Keyword::SelfType
                                    | Keyword::Super
                            )
                            | TokenKind::Dot
                    ) =>
                {
                    Some(token.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.resolve_token_path(module, &direct_tokens, generics, self_allowed, node.span);
        for child in &node.children {
            let SyntaxElement::Node(child) = child else {
                continue;
            };
            match child.kind {
                SyntaxKind::Type => self.resolve_type(child, module, generics, self_allowed),
                SyntaxKind::TypeArguments => {
                    for argument in child_nodes(child) {
                        if argument.kind == SyntaxKind::Type {
                            self.resolve_type(argument, module, generics, self_allowed);
                        }
                    }
                }
                _ => {
                    let mut scopes = LexicalScopes::default();
                    scopes.push();
                    self.resolve_expression(child, module, generics, &scopes, self_allowed);
                }
            }
        }
    }

    pub(super) fn resolve_token_path(
        &mut self,
        module: ModuleId,
        tokens: &[Token],
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        _fallback: Span,
    ) -> Option<NameTarget> {
        let meaningful = tokens
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Dot))
            .collect::<Vec<_>>();
        let first = meaningful.first().copied()?;
        let module = self.syntax_context_module(first.span, module);
        let span = Span::new(
            first.span.file,
            first.span.start,
            meaningful
                .last()
                .map_or(first.span.end, |last| last.span.end),
        );
        if meaningful.len() == 1 {
            match &first.kind {
                TokenKind::Identifier(name) => {
                    let symbol = self.intern(name);
                    if let Some(parameter) = generics.get(&symbol).copied() {
                        let target = NameTarget::GenericParameter(parameter);
                        self.record_reference(span, target, Vec::new());
                        return Some(target);
                    }
                }
                TokenKind::Keyword(Keyword::SelfType) => {
                    if self_allowed {
                        self.record_reference(span, NameTarget::SelfType, Vec::new());
                        return Some(NameTarget::SelfType);
                    }
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::NameResolution,
                            "`Self` is valid only in a struct, trait, or trait implementation",
                        )
                        .with_primary(span),
                    );
                    return None;
                }
                _ => {}
            }
        }
        let parts = path_parts_from_tokens(tokens, &mut self.program.symbols);
        let result = self.resolve_module_path(module, &parts, span)?;
        let target = NameTarget::Item(result.item);
        self.record_reference(span, target, result.provenance);
        Some(target)
    }

    pub(super) fn resolve_function_body(
        &mut self,
        _declaration: DeclarationId,
        syntax: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let mut scopes = LexicalScopes::default();
        scopes.push();
        if let Some(parameters) = child_nodes(syntax)
            .into_iter()
            .find(|node| node.kind == SyntaxKind::Parameters)
        {
            for parameter in child_nodes(parameters)
                .into_iter()
                .filter(|node| node.kind == SyntaxKind::Parameter)
            {
                if let Some(token) = parameter_name_token(parameter) {
                    self.declare_local(&mut scopes, token, LocalBindingKind::Parameter);
                }
            }
        }
        if let Some(block) = child_nodes(syntax)
            .into_iter()
            .find(|node| node.kind == SyntaxKind::Block)
        {
            self.resolve_block(block, module, generics, &mut scopes, self_allowed, false);
        }
    }

    pub(super) fn resolve_block(
        &mut self,
        block: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
        nested: bool,
    ) {
        if nested {
            scopes.push();
        }
        for statement in child_nodes(block) {
            self.resolve_statement(statement, module, generics, scopes, self_allowed);
        }
        if nested {
            scopes.pop();
        }
    }

    pub(super) fn resolve_statement(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
    ) {
        match node.kind {
            SyntaxKind::LetStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Type {
                        self.resolve_type(child, module, generics, self_allowed);
                    } else if child.kind == SyntaxKind::TuplePattern {
                        // Binding names enter scope together only after the
                        // initializer has been resolved.
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
                for token in binding_name_tokens(node) {
                    self.declare_local(scopes, token, LocalBindingKind::Local);
                }
            }
            SyntaxKind::IfStatement => {
                for child in child_nodes(node) {
                    match child.kind {
                        SyntaxKind::Block => {
                            self.resolve_block(child, module, generics, scopes, self_allowed, true)
                        }
                        SyntaxKind::ElseClause => {
                            if let Some(block) = child_nodes(child)
                                .into_iter()
                                .find(|node| node.kind == SyntaxKind::Block)
                            {
                                self.resolve_block(
                                    block,
                                    module,
                                    generics,
                                    scopes,
                                    self_allowed,
                                    true,
                                );
                            }
                        }
                        _ => {
                            self.resolve_expression(child, module, generics, scopes, self_allowed);
                        }
                    }
                }
            }
            SyntaxKind::MatchStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        for arm in child_nodes(child)
                            .into_iter()
                            .filter(|node| node.kind == SyntaxKind::MatchArm)
                        {
                            self.resolve_match_arm(arm, module, generics, scopes, self_allowed);
                        }
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::ForStatement => {
                let children = child_nodes(node);
                for child in &children {
                    if child.kind != SyntaxKind::Block {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
                scopes.push();
                if let Some(token) = for_binding_token(node) {
                    self.declare_local(scopes, token, LocalBindingKind::Loop);
                }
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.resolve_block(block, module, generics, scopes, self_allowed, false);
                }
                scopes.pop();
            }
            SyntaxKind::WhileStatement | SyntaxKind::UnsafeBlock => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        self.resolve_block(child, module, generics, scopes, self_allowed, true);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::AssignmentStatement
            | SyntaxKind::ExpressionStatement
            | SyntaxKind::ReturnStatement => {
                for child in child_nodes(node) {
                    self.resolve_expression(child, module, generics, scopes, self_allowed);
                }
            }
            // `defer:` defers a block, whose body is its own lexical scope;
            // `defer call` defers one call expression.
            SyntaxKind::DeferStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        self.resolve_block(child, module, generics, scopes, self_allowed, true);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::ExpectStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        self.resolve_block(child, module, generics, scopes, self_allowed, true);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement
            | SyntaxKind::PassStatement => {}
            _ => {
                for child in child_nodes(node) {
                    self.resolve_expression(child, module, generics, scopes, self_allowed);
                }
            }
        }
    }

    pub(super) fn resolve_match_arm(
        &mut self,
        arm: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
    ) {
        scopes.push();
        if let Some(pattern) = child_nodes(arm).into_iter().find(|node| {
            matches!(
                node.kind,
                SyntaxKind::Pattern
                    | SyntaxKind::AlternativePattern
                    | SyntaxKind::DereferencePattern
                    | SyntaxKind::TuplePattern
                    | SyntaxKind::RecordPattern
                    | SyntaxKind::VariantPattern
            )
        }) {
            let mut bindings = BTreeMap::<Symbol, Span>::new();
            self.collect_pattern_bindings(pattern, module, generics, self_allowed, &mut bindings);
            for (name, span) in bindings {
                self.declare_local_symbol(scopes, name, span, LocalBindingKind::PatternCandidate);
            }
        }
        for child in child_nodes(arm) {
            match child.kind {
                SyntaxKind::Guard => {
                    for expression in child_nodes(child) {
                        self.resolve_expression(expression, module, generics, scopes, self_allowed);
                    }
                }
                SyntaxKind::Block => {
                    self.resolve_block(child, module, generics, scopes, self_allowed, false)
                }
                _ => {}
            }
        }
        scopes.pop();
    }

    pub(super) fn collect_pattern_bindings(
        &mut self,
        pattern: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        bindings: &mut BTreeMap<Symbol, Span>,
    ) {
        match pattern.kind {
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                let path_tokens = leading_pattern_path(pattern);
                self.resolve_pattern_path(
                    module,
                    &path_tokens,
                    generics,
                    self_allowed,
                    pattern.span,
                );
                for child in child_nodes(pattern) {
                    if child.kind == SyntaxKind::PatternField {
                        let nested = child_nodes(child);
                        if nested.is_empty() {
                            if let Some(token) = first_identifier(child) {
                                let name = self.intern(token_text(token));
                                bindings.entry(name).or_insert(token.span);
                            }
                        } else {
                            for nested in nested {
                                self.collect_pattern_bindings(
                                    nested,
                                    module,
                                    generics,
                                    self_allowed,
                                    bindings,
                                );
                            }
                        }
                    } else if matches!(
                        child.kind,
                        SyntaxKind::Pattern
                            | SyntaxKind::AlternativePattern
                            | SyntaxKind::DereferencePattern
                            | SyntaxKind::TuplePattern
                            | SyntaxKind::RecordPattern
                            | SyntaxKind::VariantPattern
                    ) {
                        self.collect_pattern_bindings(
                            child,
                            module,
                            generics,
                            self_allowed,
                            bindings,
                        );
                    }
                }
            }
            SyntaxKind::TuplePattern
            | SyntaxKind::AlternativePattern
            | SyntaxKind::DereferencePattern => {
                for child in child_nodes(pattern) {
                    self.collect_pattern_bindings(child, module, generics, self_allowed, bindings);
                }
            }
            SyntaxKind::Pattern => {
                let nested = child_nodes(pattern);
                if !nested.is_empty() {
                    for child in nested {
                        self.collect_pattern_bindings(
                            child,
                            module,
                            generics,
                            self_allowed,
                            bindings,
                        );
                    }
                    return;
                }
                let identifiers = pattern
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        SyntaxElement::Token(token)
                            if matches!(token.kind, TokenKind::Identifier(_)) =>
                        {
                            Some(token)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
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
                    let tokens = pattern
                        .children
                        .iter()
                        .filter_map(|child| match child {
                            SyntaxElement::Token(token) => Some(token.clone()),
                            SyntaxElement::Node(_) => None,
                        })
                        .collect::<Vec<_>>();
                    self.resolve_pattern_path(
                        module,
                        &tokens,
                        generics,
                        self_allowed,
                        pattern.span,
                    );
                } else if let [token] = identifiers.as_slice() {
                    let name = self.intern(token_text(token));
                    if self.program.symbol_text(name) != "_" {
                        bindings.entry(name).or_insert(token.span);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn resolve_pattern_path(
        &mut self,
        module: ModuleId,
        tokens: &[Token],
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        fallback: Span,
    ) -> Option<NameTarget> {
        let meaningful = tokens
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Dot))
            .collect::<Vec<_>>();
        let first = meaningful.first().copied()?;
        let module = self.syntax_context_module(first.span, module);
        if meaningful.len() == 1 {
            return self.resolve_token_path(module, tokens, generics, self_allowed, fallback);
        }
        let parts = path_parts_from_tokens(tokens, &mut self.program.symbols);
        let mut latest = None;
        for end in 1..=parts.len() {
            let span = Span::new(
                first.span.file,
                first.span.start,
                meaningful
                    .get(end.saturating_sub(1))
                    .map_or(first.span.end, |token| token.span.end),
            );
            let result = self.resolve_module_path(module, &parts[..end], span)?;
            let is_module = matches!(result.item, ItemId::Module(_));
            latest = Some((span, result));
            if !is_module {
                break;
            }
        }
        let (span, result) = latest?;
        let target = NameTarget::Item(result.item);
        self.record_reference(span, target, result.provenance);
        Some(target)
    }

    pub(super) fn resolve_expression(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &LexicalScopes,
        self_allowed: bool,
    ) -> Option<NameTarget> {
        match node.kind {
            SyntaxKind::ClosureExpression => {
                self.resolve_closure_expression(node, module, generics, scopes, self_allowed);
                None
            }
            SyntaxKind::NameExpression => {
                let token = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => Some(token),
                    SyntaxElement::Node(_) => None,
                })?;
                self.resolve_unqualified_name(token, module, generics, scopes, self_allowed)
            }
            SyntaxKind::MemberExpression => {
                let base = child_nodes(node).first().copied().and_then(|child| {
                    self.resolve_expression(child, module, generics, scopes, self_allowed)
                });
                let member = node.children.iter().rev().find_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        Some(token)
                    }
                    _ => None,
                });
                if let (Some(NameTarget::Item(ItemId::Module(base))), Some(member)) = (base, member)
                {
                    let name = self.intern(token_text(member));
                    let context = self.syntax_context_module(member.span, module);
                    let require_public = self.module_access_is_external(context, base);
                    let result =
                        self.lookup_module_name(base, name, require_public, member.span)?;
                    let target = NameTarget::Item(result.item);
                    self.record_reference(member.span, target, result.provenance);
                    Some(target)
                } else {
                    None
                }
            }
            SyntaxKind::Type => {
                self.resolve_type(node, module, generics, self_allowed);
                None
            }
            SyntaxKind::RecordField if child_nodes(node).is_empty() => {
                let token = first_identifier(node)?;
                self.resolve_unqualified_name(token, module, generics, scopes, self_allowed)
            }
            _ => {
                let mut result = None;
                for child in child_nodes(node) {
                    result = self
                        .resolve_expression(child, module, generics, scopes, self_allowed)
                        .or(result);
                }
                if node.kind == SyntaxKind::ParenthesizedExpression {
                    result
                } else {
                    None
                }
            }
        }
    }

    fn resolve_closure_expression(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        outer_scopes: &LexicalScopes,
        self_allowed: bool,
    ) {
        let declaration = DeclarationId(self.program.declarations.len() as u32);
        let name = self.intern(&format!("$closure{}", declaration.index()));
        let mut generic_parameters = generics.values().copied().collect::<Vec<_>>();
        generic_parameters.sort();
        generic_parameters.dedup();
        self.program.declarations.push(Declaration {
            id: declaration,
            module,
            name,
            kind: DeclarationKind::Closure,
            visibility: Visibility::Package,
            span: node.span,
            syntax: node.clone(),
            parent_declaration: None,
            parent_impl: None,
            generic_parameters,
            externally_reachable: false,
            foreign_binding: None,
            test_selected: false,
        });

        let mut scopes = LexicalScopes::default();
        scopes.push();
        let mut captures = Vec::new();
        let mut captured_sources = BTreeSet::new();
        if let Some(list) = child_nodes(node)
            .into_iter()
            .find(|child| child.kind == SyntaxKind::ClosureCaptureList)
        {
            for capture in child_nodes(list)
                .into_iter()
                .filter(|child| child.kind == SyntaxKind::ClosureCapture)
            {
                let identifiers = capture
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        SyntaxElement::Token(
                            token @ Token {
                                kind: TokenKind::Identifier(_),
                                ..
                            },
                        ) => Some(token),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let Some(source_token) = identifiers.first().copied() else {
                    continue;
                };
                let target = self.resolve_unqualified_name(
                    source_token,
                    module,
                    generics,
                    outer_scopes,
                    self_allowed,
                );
                let Some(NameTarget::Local(source)) = target else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::NameResolution,
                            "a closure capture must name an enclosing local binding",
                        )
                        .with_primary(source_token.span),
                    );
                    continue;
                };
                if !captured_sources.insert(source) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::DeclarationConflict,
                            "an enclosing local may be captured only once",
                        )
                        .with_primary(source_token.span)
                        .with_related(
                            self.program.local_bindings[source.index()].span,
                            "local declared here",
                        ),
                    );
                    continue;
                }
                let binding_token = identifiers.get(1).copied().unwrap_or(source_token);
                let binding = self.declare_local(
                    &mut scopes,
                    binding_token,
                    LocalBindingKind::ClosureCapture,
                );
                let direct = capture.direct_tokens();
                let kind = if direct
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Amp))
                {
                    if direct
                        .iter()
                        .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Var)))
                    {
                        ClosureCaptureKind::MutableReference
                    } else {
                        ClosureCaptureKind::SharedReference
                    }
                } else if direct
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Star))
                {
                    if direct
                        .iter()
                        .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Var)))
                    {
                        ClosureCaptureKind::MutableRawPointer
                    } else {
                        ClosureCaptureKind::SharedRawPointer
                    }
                } else {
                    ClosureCaptureKind::Value
                };
                captures.push(ClosureCapture {
                    source,
                    binding,
                    kind,
                    span: capture.span,
                });
            }
        }

        if let Some(parameters) = child_nodes(node)
            .into_iter()
            .find(|child| child.kind == SyntaxKind::Parameters)
        {
            for parameter in child_nodes(parameters)
                .into_iter()
                .filter(|child| child.kind == SyntaxKind::Parameter)
            {
                if let Some(type_node) = child_nodes(parameter)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Type)
                {
                    self.resolve_type(type_node, module, generics, self_allowed);
                }
                if let Some(token) = parameter_name_token(parameter) {
                    self.declare_local(&mut scopes, token, LocalBindingKind::Parameter);
                }
            }
        }
        for type_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::Type)
        {
            self.resolve_type(type_node, module, generics, self_allowed);
        }
        if let Some(block) = child_nodes(node)
            .into_iter()
            .find(|child| child.kind == SyntaxKind::Block)
        {
            self.resolve_block(block, module, generics, &mut scopes, self_allowed, false);
        }
        self.program.closures.push(ResolvedClosure {
            declaration,
            span: node.span,
            captures,
        });
    }

    pub(super) fn resolve_unqualified_name(
        &mut self,
        token: &Token,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &LexicalScopes,
        self_allowed: bool,
    ) -> Option<NameTarget> {
        let (name, special) = match &token.kind {
            TokenKind::Identifier(name) => (self.intern(name), None),
            TokenKind::Keyword(Keyword::SelfValue) => {
                (self.intern("self"), Some(Keyword::SelfValue))
            }
            TokenKind::Keyword(Keyword::SelfType) => (self.intern("Self"), Some(Keyword::SelfType)),
            TokenKind::Keyword(Keyword::Root) => (self.intern("root"), Some(Keyword::Root)),
            TokenKind::Keyword(Keyword::Super) => (self.intern("super"), Some(Keyword::Super)),
            _ => return None,
        };
        if special == Some(Keyword::SelfValue) {
            if let Some(local) = scopes.lookup(name) {
                let target = NameTarget::Local(local);
                self.record_reference(token.span, target, Vec::new());
                return Some(target);
            }
            // Outside a receiver-bearing method, `self` is the current
            // module path root. A receiver binding deliberately shadows it.
            let target = NameTarget::Item(ItemId::Module(module));
            self.record_reference(token.span, target, Vec::new());
            return Some(target);
        }
        if special == Some(Keyword::SelfType) {
            if self_allowed {
                self.record_reference(token.span, NameTarget::SelfType, Vec::new());
                return Some(NameTarget::SelfType);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::NameResolution,
                    "`Self` is valid only in a struct, trait, or trait implementation",
                )
                .with_primary(token.span),
            );
            return None;
        }
        if let Some(local) = scopes.lookup(name) {
            let target = NameTarget::Local(local);
            self.record_reference(token.span, target, Vec::new());
            return Some(target);
        }
        if let Some(parameter) = generics.get(&name).copied() {
            let target = NameTarget::GenericParameter(parameter);
            self.record_reference(token.span, target, Vec::new());
            return Some(target);
        }
        let module = self.syntax_context_module(token.span, module);
        let result = match special {
            Some(Keyword::Root) => self.resolve_module_path(module, &[PathPart::Root], token.span),
            Some(Keyword::Super) => {
                self.resolve_module_path(module, &[PathPart::Super], token.span)
            }
            _ => {
                let package = self.program.modules[module.index()].package.clone();
                if let Some(package) = package {
                    if let Some(dependency) = self
                        .graph
                        .dependency(&package, self.program.symbol_text(name))
                    {
                        Some(LookupResult {
                            item: ItemId::Module(self.program.package_roots[dependency]),
                            provenance: Vec::new(),
                        })
                    } else if let Some(found) =
                        self.lookup_module_name(module, name, false, token.span)
                    {
                        Some(found)
                    } else {
                        self.program
                            .prelude
                            .get(&name)
                            .copied()
                            .map(|item| LookupResult {
                                item,
                                provenance: Vec::new(),
                            })
                            .or_else(|| self.standard_package_root(name))
                    }
                } else {
                    self.lookup_module_name(module, name, false, token.span)
                        .or_else(|| {
                            self.program
                                .prelude
                                .get(&name)
                                .copied()
                                .map(|item| LookupResult {
                                    item,
                                    provenance: Vec::new(),
                                })
                        })
                        .or_else(|| self.standard_package_root(name))
                }
            }
        };
        let Some(result) = result else {
            self.unresolved_name(name, token.span);
            return None;
        };
        let target = NameTarget::Item(result.item);
        self.record_reference(token.span, target, result.provenance);
        Some(target)
    }

    pub(super) fn module_access_is_external(&self, from: ModuleId, target: ModuleId) -> bool {
        match (
            &self.program.modules[from.index()].package,
            &self.program.modules[target.index()].package,
        ) {
            (Some(from), Some(target)) => from != target,
            (_, None) => true,
            _ => false,
        }
    }

    pub(super) fn declare_local(
        &mut self,
        scopes: &mut LexicalScopes,
        token: &Token,
        kind: LocalBindingKind,
    ) -> LocalBindingId {
        let name = match &token.kind {
            TokenKind::Identifier(name) => self.intern(name),
            TokenKind::Keyword(Keyword::SelfValue) => self.intern("self"),
            _ => unreachable!("binding token is an identifier or `self`"),
        };
        self.declare_local_symbol(scopes, name, token.span, kind)
    }

    pub(super) fn declare_local_symbol(
        &mut self,
        scopes: &mut LexicalScopes,
        name: Symbol,
        span: Span,
        kind: LocalBindingKind,
    ) -> LocalBindingId {
        let id = LocalBindingId(self.program.local_bindings.len() as u32);
        let current = scopes
            .scopes
            .last_mut()
            .expect("function resolution has a lexical scope");
        if let Some(previous) = current.get(&name).copied() {
            self.duplicate_diagnostic(
                "lexical binding",
                span,
                Some(self.program.local_bindings[previous.index()].span),
            );
        } else {
            current.insert(name, id);
        }
        self.program.local_bindings.push(LocalBinding {
            id,
            name,
            span,
            kind,
        });
        id
    }

    pub(super) fn record_reference(
        &mut self,
        span: Span,
        target: NameTarget,
        provenance: Vec<Span>,
    ) {
        self.program.references.push(ResolvedReference {
            span,
            target,
            provenance,
        });
    }
}
