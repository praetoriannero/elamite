//! End-to-end execution and structural rewriting for user compile-time forms.

use std::collections::BTreeSet;

use crate::diagnostics::{Category, Diagnostic};
use crate::package::{PackageGraph, PackageId};
use crate::parser::{FragmentKind, parse_fragment};
use crate::source::Span;
use crate::syntax::{Keyword, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind};

use super::interpreter::{CompileTimeType, LoweredDeclaration, Value, execute, lower_declaration};
use super::namespace::{
    CompileTimeBinding, CompileTimeDeclarationId, CompileTimeEnvironment, CompileTimeModuleId,
    CompileTimeNamespace,
};
use super::provenance::{GeneratedSource, ProvenanceTable};
use super::quote::QuoteRole;
use super::scheduler::{
    ExpansionLocation, ExpansionRequest, ExpansionRole, ExpansionScheduler, RecoveryReason,
    ScheduledOutput, StructuralInput, StructuralToken,
};
use super::{ExpandedUnit, ExpandedUnitIdentity};

pub(super) fn execute_package(
    graph: &PackageGraph,
    units: &mut [ExpandedUnit],
    environment: &CompileTimeEnvironment,
    scheduler: &mut ExpansionScheduler,
    provenance: &mut ProvenanceTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let programs = environment
        .declarations
        .iter()
        .map(|declaration| match lower_declaration(declaration) {
            Ok(program) => Some(program),
            Err(errors) => {
                diagnostics.extend(errors);
                None
            }
        })
        .collect::<Vec<_>>();
    let mut engine = Engine {
        graph,
        environment,
        programs,
        scheduler,
        provenance,
        diagnostics,
        active: BTreeSet::new(),
        generated_origins: Vec::new(),
    };
    for unit in units.iter_mut().filter(|unit| !unit.is_standard()) {
        let Some((package, path)) = unit_package_path(&unit.identity) else {
            continue;
        };
        let Some(module) = engine.environment.module(&package, &path) else {
            continue;
        };
        unit.tree = engine.transform_container(&unit.tree, module);
    }
}

struct Engine<'a> {
    graph: &'a PackageGraph,
    environment: &'a CompileTimeEnvironment,
    programs: Vec<Option<LoweredDeclaration>>,
    scheduler: &'a mut ExpansionScheduler,
    provenance: &'a mut ProvenanceTable,
    diagnostics: &'a mut Vec<Diagnostic>,
    active: BTreeSet<(CompileTimeDeclarationId, ExpansionRole, String)>,
    generated_origins: Vec<(Span, super::provenance::OriginId)>,
}

impl Engine<'_> {
    fn transform_container(
        &mut self,
        node: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> SyntaxNode {
        let mut children = Vec::new();
        for child in &node.children {
            let SyntaxElement::Node(item) = child else {
                children.push(child.clone());
                continue;
            };
            if item.kind == SyntaxKind::MacroExpression && is_user_macro(item) {
                children.extend(
                    self.expand_macro(item, module, ExpansionRole::Items)
                        .into_iter()
                        .map(|node| SyntaxElement::Node(Box::new(node))),
                );
                continue;
            }
            if item.kind == SyntaxKind::ExpressionStatement
                && let Some(invocation) = item
                    .direct_nodes()
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::MacroExpression && is_user_macro(child))
                && self.macro_return_role(invocation, module) == Some(ExpansionRole::Statements)
            {
                children.extend(
                    self.expand_macro(invocation, module, ExpansionRole::Statements)
                        .into_iter()
                        .map(|node| SyntaxElement::Node(Box::new(node))),
                );
                continue;
            }
            if is_definition(item.kind) {
                children.extend(
                    self.transform_definition(item, module)
                        .into_iter()
                        .map(|node| SyntaxElement::Node(Box::new(node))),
                );
                continue;
            }
            if item.kind == SyntaxKind::Module {
                let nested = declaration_name(item).and_then(|name| {
                    let current = &self.environment.modules[module.index()];
                    let mut path = current.path.clone();
                    path.push(name);
                    self.environment.module(&current.package, &path)
                });
                children.push(SyntaxElement::Node(Box::new(match nested {
                    Some(nested) => self.transform_node_in_module(item, nested),
                    None => item.as_ref().clone(),
                })));
                continue;
            }
            children.push(SyntaxElement::Node(Box::new(
                self.transform_node_in_module(item, module),
            )));
        }
        SyntaxNode {
            kind: node.kind,
            span: node.span,
            children,
        }
    }

    fn transform_node_in_module(
        &mut self,
        node: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> SyntaxNode {
        if matches!(node.kind, SyntaxKind::File | SyntaxKind::Block) {
            return self.transform_container(node, module);
        }
        if node.kind == SyntaxKind::MacroExpression && is_user_macro(node) {
            let role = self
                .macro_return_role(node, module)
                .unwrap_or(ExpansionRole::Expression);
            let mut output = self.expand_macro(node, module, role);
            if output.len() == 1 {
                return output.remove(0);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::CompileTime,
                    "a list-valued macro cannot be used in a scalar syntax position",
                )
                .with_primary(node.span),
            );
            return recovery_node(node.span);
        }
        let children = node
            .children
            .iter()
            .map(|child| match child {
                SyntaxElement::Token(token) => SyntaxElement::Token(token.clone()),
                SyntaxElement::Node(child)
                    if child.kind == SyntaxKind::MacroExpression && is_user_macro(child) =>
                {
                    let mut expanded = self.expand_macro(child, module, child_position(node.kind));
                    if expanded.len() == 1 {
                        SyntaxElement::Node(Box::new(expanded.remove(0)))
                    } else {
                        self.invalid_attachment(
                            child,
                            "a list-valued macro cannot be used in a scalar syntax position",
                        );
                        SyntaxElement::Node(Box::new(recovery_node(child.span)))
                    }
                }
                SyntaxElement::Node(child) => {
                    SyntaxElement::Node(Box::new(self.transform_node_in_module(child, module)))
                }
            })
            .collect();
        SyntaxNode {
            kind: node.kind,
            span: node.span,
            children,
        }
    }

    fn transform_definition(
        &mut self,
        item: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> Vec<SyntaxNode> {
        let mut current = vec![item.clone()];
        let attributes = item
            .direct_children(SyntaxKind::Attribute)
            .into_iter()
            .filter(|attribute| attachment_kind(attribute) == Some(CompileTimeNamespace::Attribute))
            .cloned()
            .collect::<Vec<_>>();
        for attribute in attributes {
            let targets = current
                .iter()
                .enumerate()
                .filter(|(_, node)| node.kind == item.kind)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if targets.len() != 1 {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "an interacting attribute requires exactly one remaining target definition",
                    )
                    .with_primary(attribute.span),
                );
                break;
            }
            let index = targets[0];
            let replacement = self.execute_attribute(&attribute, &current[index], module);
            current.splice(index..=index, replacement);
        }

        let original_target = current.iter().find(|node| node.kind == item.kind).cloned();
        let derives = item
            .direct_children(SyntaxKind::Attribute)
            .into_iter()
            .filter(|attribute| attachment_kind(attribute) == Some(CompileTimeNamespace::Derive))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(target) = original_target {
            let (generated, builtins) = self.execute_derives(&derives, &target, module);
            current = current
                .into_iter()
                .map(|node| strip_custom_attachments(&node))
                .collect();
            if !builtins.is_empty() {
                if let Some(index) = current.iter().position(|node| is_definition(node.kind)) {
                    current[index] = attach_builtin_derives(&current[index], &builtins);
                }
            }
            current.extend(generated);
        }

        current
            .into_iter()
            .flat_map(|node| {
                if is_definition(node.kind) {
                    vec![self.transform_node_in_module(&node, module)]
                } else if node.kind == SyntaxKind::MacroExpression {
                    self.expand_macro(&node, module, ExpansionRole::Items)
                } else {
                    vec![self.transform_node_in_module(&node, module)]
                }
            })
            .collect()
    }

    fn execute_attribute(
        &mut self,
        attachment: &SyntaxNode,
        target: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> Vec<SyntaxNode> {
        let Some(call) = attachment_call(attachment) else {
            self.invalid_attachment(attachment, "malformed attribute attachment");
            return vec![target.clone()];
        };
        let Some(declaration) = self.resolve(module, CompileTimeNamespace::Attribute, &call.path)
        else {
            self.unresolved(attachment.span, "attribute", &call.path.join("."));
            return vec![target.clone()];
        };
        if !self.declaration_visible(module, declaration) {
            self.private_declaration(attachment, declaration);
            return vec![target.clone()];
        }
        let Some(program) = self.programs[declaration.index()].as_ref() else {
            return vec![target.clone()];
        };
        let Some(role) = definition_role(target.kind) else {
            self.invalid_attachment(
                attachment,
                "this definition kind cannot receive a structural attribute",
            );
            return vec![target.clone()];
        };
        if !matches!(program.parameters.first().map(|parameter| &parameter.ty), Some(CompileTimeType::Ast(expected)) if *expected == role)
        {
            self.invalid_attachment(
                attachment,
                "attribute target type does not match the attached definition",
            );
            return vec![target.clone()];
        }
        let mut arguments = vec![Value::syntax(role, target.clone())];
        let explicit = &program.parameters[1..];
        match parse_arguments(&call.arguments, explicit, attachment.span) {
            Ok(values) => arguments.extend(values),
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                return vec![target.clone()];
            }
        }
        match self.run(
            declaration,
            arguments,
            attachment,
            module,
            ExpansionRole::Attribute,
        ) {
            Some(Value::Syntax {
                role: output_role,
                nodes,
            }) if output_role == role || output_role == QuoteRole::ItemList => {
                if nodes
                    .iter()
                    .any(contains_generated_compile_time_declaration)
                {
                    self.invalid_attachment(
                        attachment,
                        "generated syntax cannot declare or import compile-time forms",
                    );
                    vec![target.clone()]
                } else {
                    nodes
                }
            }
            Some(_) => {
                self.invalid_attachment(
                    attachment,
                    "attribute returned a value with the wrong structural role",
                );
                vec![target.clone()]
            }
            None => vec![target.clone()],
        }
    }

    fn execute_derives(
        &mut self,
        attachments: &[SyntaxNode],
        target: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> (Vec<SyntaxNode>, Vec<Token>) {
        let mut generated = Vec::new();
        let mut builtins = Vec::new();
        for attachment in attachments {
            for path in derive_paths(attachment) {
                let Some(declaration) = self.resolve(module, CompileTimeNamespace::Derive, &path)
                else {
                    builtins.extend(path_tokens(attachment, &path));
                    continue;
                };
                if !self.declaration_visible(module, declaration) {
                    self.private_declaration(attachment, declaration);
                    continue;
                }
                let Some(role) = definition_role(target.kind) else {
                    self.invalid_attachment(attachment, "derive target must be a struct or enum");
                    continue;
                };
                if !matches!(
                    role,
                    QuoteRole::StructDefinition | QuoteRole::EnumDefinition
                ) {
                    self.invalid_attachment(attachment, "derive target must be a struct or enum");
                    continue;
                }
                if let Some(Value::Syntax {
                    role: QuoteRole::Implementation,
                    nodes,
                }) = self.run(
                    declaration,
                    vec![Value::syntax(role, target.clone())],
                    attachment,
                    module,
                    ExpansionRole::Derive,
                ) {
                    for node in nodes {
                        if node.kind != SyntaxKind::Impl {
                            self.invalid_attachment(
                                attachment,
                                "derive returned a non-implementation item",
                            );
                            continue;
                        }
                        if !derive_output_matches(
                            &node,
                            &self.environment.declarations[declaration.index()].trait_path,
                            target,
                        ) {
                            self.invalid_attachment(
                                attachment,
                                "derive implementation must name its declared trait and exact attached target",
                            );
                            continue;
                        }
                        generated.push(node);
                    }
                }
            }
        }
        (generated, builtins)
    }

    fn expand_macro(
        &mut self,
        invocation: &SyntaxNode,
        module: CompileTimeModuleId,
        position: ExpansionRole,
    ) -> Vec<SyntaxNode> {
        let Some(call) = macro_call(invocation) else {
            self.invalid_attachment(invocation, "malformed user macro invocation");
            return vec![recovery_node(invocation.span)];
        };
        let Some(declaration) = self.resolve(module, CompileTimeNamespace::Macro, &call.path)
        else {
            self.unresolved(invocation.span, "macro", &call.path.join("."));
            return vec![recovery_node(invocation.span)];
        };
        if !self.declaration_visible(module, declaration) {
            self.private_declaration(invocation, declaration);
            return vec![recovery_node(invocation.span)];
        }
        let Some(program) = self.programs[declaration.index()].as_ref() else {
            return vec![recovery_node(invocation.span)];
        };
        let Some(role) = expansion_role(&program.return_type) else {
            self.invalid_attachment(invocation, "macro return type is not expandable");
            return vec![recovery_node(invocation.span)];
        };
        if role != position {
            self.invalid_attachment(
                invocation,
                format!("macro produces {role:?} syntax but is used in a {position:?} position"),
            );
            return vec![recovery_node(invocation.span)];
        }
        let arguments = match parse_arguments(&call.arguments, &program.parameters, invocation.span)
        {
            Ok(arguments) => arguments,
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                return vec![recovery_node(invocation.span)];
            }
        };
        let key = (declaration, role, structural_text(&call.arguments));
        if !self.active.insert(key.clone()) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::CompileTime,
                    "compile-time expansion cycle detected",
                )
                .with_primary(invocation.span)
                .with_related(program.span, "re-entered declaration is here"),
            );
            return vec![recovery_node(invocation.span)];
        }
        let result = self.run(declaration, arguments, invocation, module, role);
        let mut nodes = match result.and_then(Value::into_syntax) {
            Some((_, nodes)) => nodes,
            None => vec![recovery_node(invocation.span)],
        };
        if nodes
            .iter()
            .any(contains_generated_compile_time_declaration)
        {
            self.invalid_attachment(
                invocation,
                "generated syntax cannot declare or import compile-time forms",
            );
            self.active.remove(&key);
            return vec![recovery_node(invocation.span)];
        }
        nodes = nodes
            .into_iter()
            .flat_map(|node| match role {
                ExpansionRole::Items | ExpansionRole::Statements => self
                    .transform_container(
                        &SyntaxNode {
                            kind: if role == ExpansionRole::Items {
                                SyntaxKind::File
                            } else {
                                SyntaxKind::Block
                            },
                            span: node.span,
                            children: vec![SyntaxElement::Node(Box::new(node))],
                        },
                        module,
                    )
                    .direct_nodes()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
                _ => vec![self.transform_node_in_module(&node, module)],
            })
            .collect();
        self.active.remove(&key);
        nodes
    }

    fn macro_return_role(
        &self,
        invocation: &SyntaxNode,
        module: CompileTimeModuleId,
    ) -> Option<ExpansionRole> {
        let call = macro_call(invocation)?;
        let declaration = self.resolve(module, CompileTimeNamespace::Macro, &call.path)?;
        expansion_role(&self.programs[declaration.index()].as_ref()?.return_type)
    }

    fn run(
        &mut self,
        declaration: CompileTimeDeclarationId,
        arguments: Vec<Value>,
        invocation: &SyntaxNode,
        module: CompileTimeModuleId,
        role: ExpansionRole,
    ) -> Option<Value> {
        let program = self.programs[declaration.index()].as_ref()?;
        let invocation_token = all_tokens(invocation).first().copied()?;
        let invocation_origin = self
            .generated_origins
            .iter()
            .rev()
            .find_map(|(span, origin)| (*span == invocation_token.span).then_some(*origin))
            .or_else(|| self.provenance.origin_for_physical(invocation_token.span))?;
        let definition_span = self.environment.declarations[declaration.index()].span;
        let definition_origin = self.provenance.origin_for_physical(definition_span)?;
        let module_record = &self.environment.modules[module.index()];
        let input_tokens = macro_call(invocation).map_or_else(Vec::new, |call| call.arguments);
        let request = ExpansionRequest {
            declaration,
            role,
            input: StructuralInput {
                tokens: input_tokens
                    .iter()
                    .map(|token| StructuralToken {
                        kind: token.kind.clone(),
                        source: token_source(&token.kind),
                    })
                    .collect(),
            },
            location: ExpansionLocation::physical(
                module_record.package.clone(),
                module_record.path.clone(),
                invocation.span.start,
            ),
            invocation: invocation_origin,
            definition: definition_origin,
        };
        let work = self.scheduler.enqueue(request);
        let mut arguments = Some(arguments);
        let mut result = None;
        let mut failure = None;
        self.scheduler.run(|scheduled, resources| {
            if scheduled.id != work {
                return Err(RecoveryReason::Dependency);
            }
            match execute(
                program,
                arguments.take().expect("scheduled once"),
                resources,
            ) {
                Ok(value) => {
                    let nodes = match &value {
                        Value::Syntax { nodes, .. } => nodes.iter().map(count_nodes).sum(),
                        _ => 0,
                    };
                    result = Some(value);
                    Ok(ScheduledOutput {
                        generated_nodes: nodes,
                        ..ScheduledOutput::default()
                    })
                }
                Err(error) => {
                    failure = Some(error);
                    Err(RecoveryReason::ExecutionFailure)
                }
            }
        });
        if let Some(error) = failure {
            let mut diagnostic = error
                .diagnostic(program)
                .with_related(invocation.span, "compile-time invocation is here");
            for frame in self.provenance.trace(invocation_origin).expansions {
                if let Some(span) = self.provenance.physical_span(frame.invocation) {
                    diagnostic = diagnostic.with_related(span, "enclosing expansion invoked here");
                }
                if let Some(span) = self.provenance.physical_span(frame.definition) {
                    diagnostic = diagnostic.with_related(span, "enclosing declaration is here");
                }
            }
            self.diagnostics.push(diagnostic);
            return None;
        }
        let Some(value) = result else {
            if let Some(problem) = self
                .scheduler
                .problems()
                .iter()
                .find(|problem| problem.work == work)
            {
                self.diagnostics.push(
                    Diagnostic::new(Category::CompileTime, problem.message())
                        .with_primary(invocation.span)
                        .with_related(program.span, "compile-time declaration is here"),
                );
            }
            return None;
        };
        let expansion = self
            .provenance
            .register_expansion(invocation_origin, definition_origin);
        if let Value::Syntax { nodes, .. } = &value {
            let invocation_spans = all_tokens(invocation)
                .into_iter()
                .map(|token| token.span)
                .collect::<BTreeSet<_>>();
            for token in nodes.iter().flat_map(all_tokens) {
                let physical = self
                    .provenance
                    .origin_for_physical(token.span)
                    .unwrap_or(definition_origin);
                let source = if invocation_spans.contains(&token.span) {
                    GeneratedSource::Invocation(physical)
                } else {
                    GeneratedSource::Definition(physical)
                };
                let origin = self.provenance.register_generated(expansion, source);
                self.generated_origins.push((token.span, origin));
            }
        }
        Some(value)
    }

    fn resolve(
        &self,
        source: CompileTimeModuleId,
        namespace: CompileTimeNamespace,
        path: &[String],
    ) -> Option<CompileTimeDeclarationId> {
        let name = path.last()?;
        let mut module = source;
        if path.len() > 1 {
            let source_module = &self.environment.modules[source.index()];
            let mut package = source_module.package.clone();
            let mut module_path = Vec::new();
            let mut index = 0;
            match path[0].as_str() {
                "root" => index = 1,
                "self" => {
                    module_path = source_module.path.clone();
                    index = 1;
                }
                "super" => {
                    module = source_module.parent?;
                    module_path = self.environment.modules[module.index()].path.clone();
                    index = 1;
                }
                first => {
                    if let Some(dependency) = self.graph.dependency(&package, first) {
                        package = dependency.clone();
                        index = 1;
                    } else {
                        module_path = source_module.path.clone();
                    }
                }
            }
            module_path.extend(path[index..path.len() - 1].iter().cloned());
            module = self.environment.module(&package, &module_path)?;
        }
        match self.environment.lookup(module, namespace, name)? {
            CompileTimeBinding::Declaration(declaration) => Some(declaration),
            CompileTimeBinding::Import(import) => self.environment.imports[import.index()].target,
        }
    }

    fn unresolved(&mut self, span: Span, kind: &str, path: &str) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::NameResolution,
                format!("cannot resolve {kind} `{path}`"),
            )
            .with_primary(span),
        );
    }

    fn declaration_visible(
        &self,
        source: CompileTimeModuleId,
        declaration: CompileTimeDeclarationId,
    ) -> bool {
        let source_package = &self.environment.modules[source.index()].package;
        let declaration = &self.environment.declarations[declaration.index()];
        let target_package = &self.environment.modules[declaration.module.index()].package;
        source_package == target_package
            || declaration.visibility == super::namespace::CompileTimeVisibility::Public
    }

    fn private_declaration(
        &mut self,
        invocation: &SyntaxNode,
        declaration: CompileTimeDeclarationId,
    ) {
        let declaration = &self.environment.declarations[declaration.index()];
        self.diagnostics.push(
            Diagnostic::new(
                Category::Visibility,
                format!(
                    "compile-time declaration `{}` is package-private",
                    declaration.name
                ),
            )
            .with_primary(invocation.span)
            .with_related(declaration.span, "package-private declaration is here"),
        );
    }

    fn invalid_attachment(&mut self, node: &SyntaxNode, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(Category::CompileTime, message).with_primary(node.span));
    }
}

#[derive(Debug)]
struct RawCall {
    path: Vec<String>,
    arguments: Vec<Token>,
}

fn macro_call(node: &SyntaxNode) -> Option<RawCall> {
    let tokens = node.direct_tokens();
    let open = tokens
        .iter()
        .position(|token| token.kind == TokenKind::LParen)?;
    let close = tokens
        .iter()
        .rposition(|token| token.kind == TokenKind::RParen)?;
    let path = tokens[1..open]
        .iter()
        .filter_map(|token| path_component(&token.kind))
        .collect::<Vec<_>>();
    Some(RawCall {
        path,
        arguments: tokens[open + 1..close]
            .iter()
            .map(|token| (*token).clone())
            .collect(),
    })
}

fn attachment_call(node: &SyntaxNode) -> Option<RawCall> {
    let tokens = node.direct_tokens();
    let outer_open = tokens
        .iter()
        .position(|token| token.kind == TokenKind::LParen)?;
    let outer_close = tokens
        .iter()
        .rposition(|token| token.kind == TokenKind::RParen)?;
    let inner = &tokens[outer_open + 1..outer_close];
    let call_open = inner
        .iter()
        .position(|token| token.kind == TokenKind::LParen);
    let path_end = call_open.unwrap_or(inner.len());
    let path = inner[..path_end]
        .iter()
        .filter_map(|token| path_component(&token.kind))
        .collect::<Vec<_>>();
    let arguments = call_open.map_or_else(Vec::new, |open| {
        inner[open + 1..inner.len().saturating_sub(1)]
            .iter()
            .map(|token| (*token).clone())
            .collect()
    });
    Some(RawCall { path, arguments })
}

fn parse_arguments(
    tokens: &[Token],
    parameters: &[super::interpreter::CompileTimeParameter],
    span: Span,
) -> Result<Vec<Value>, Diagnostic> {
    let parts = split_arguments(tokens);
    let fixed = parameters
        .iter()
        .take_while(|parameter| !parameter.variadic)
        .count();
    let variadic = parameters
        .last()
        .is_some_and(|parameter| parameter.variadic);
    if (!variadic && parts.len() != fixed) || (variadic && parts.len() < fixed) {
        return Err(Diagnostic::new(
            Category::CompileTime,
            format!(
                "compile-time invocation supplied {} arguments but expects {}{}",
                parts.len(),
                fixed,
                if variadic { " or more" } else { "" }
            ),
        )
        .with_primary(span));
    }
    let mut values = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        let parameter = parameters
            .get(index)
            .or_else(|| parameters.last())
            .expect("arity checked");
        let ty = if parameter.variadic {
            match &parameter.ty {
                CompileTimeType::Sequence(element) => element.as_ref(),
                _ => &parameter.ty,
            }
        } else {
            &parameter.ty
        };
        values.push(parse_argument(&part, ty, span)?);
    }
    Ok(values)
}

fn parse_argument(tokens: &[Token], ty: &CompileTimeType, span: Span) -> Result<Value, Diagnostic> {
    match ty {
        CompileTimeType::Ast(role) => {
            let kind = match role {
                QuoteRole::Expression => FragmentKind::Expression,
                QuoteRole::Pattern => FragmentKind::Pattern,
                QuoteRole::TypeSyntax => FragmentKind::Type,
                QuoteRole::StatementList => FragmentKind::Statement,
                QuoteRole::Item
                | QuoteRole::ItemList
                | QuoteRole::StructDefinition
                | QuoteRole::EnumDefinition
                | QuoteRole::FunctionDefinition
                | QuoteRole::Implementation => FragmentKind::Item,
                QuoteRole::MemberList | QuoteRole::FieldDefinition => {
                    return Err(Diagnostic::new(
                        Category::CompileTime,
                        "member syntax cannot be passed at this invocation position",
                    )
                    .with_primary(span));
                }
            };
            let output = parse_tokens(tokens, kind, span);
            if let Some(diagnostic) = output.diagnostics.into_iter().next() {
                return Err(Diagnostic::new(
                    Category::CompileTime,
                    format!("invalid macro argument: {}", diagnostic.message),
                )
                .with_primary(diagnostic.primary.unwrap_or(span)));
            }
            let actual_role = match role {
                QuoteRole::StatementList => QuoteRole::StatementList,
                QuoteRole::ItemList => QuoteRole::ItemList,
                _ => *role,
            };
            Ok(Value::syntax(actual_role, output.tree))
        }
        CompileTimeType::String => match tokens {
            [
                Token {
                    kind: TokenKind::StringLiteral(value),
                    ..
                },
            ] => Ok(Value::String(value.clone())),
            _ => Err(Diagnostic::new(
                Category::CompileTime,
                "this compile-time argument must be a string literal",
            )
            .with_primary(span)),
        },
        CompileTimeType::Bool => match tokens {
            [
                Token {
                    kind: TokenKind::Keyword(Keyword::True),
                    ..
                },
            ] => Ok(Value::Bool(true)),
            [
                Token {
                    kind: TokenKind::Keyword(Keyword::False),
                    ..
                },
            ] => Ok(Value::Bool(false)),
            _ => Err(Diagnostic::new(
                Category::CompileTime,
                "this compile-time argument must be a bool literal",
            )
            .with_primary(span)),
        },
        CompileTimeType::Integer => match tokens {
            [
                Token {
                    kind: TokenKind::IntegerLiteral { raw, .. },
                    ..
                },
            ] => raw
                .replace('_', "")
                .parse::<i128>()
                .map(Value::Integer)
                .map_err(|_| {
                    Diagnostic::new(
                        Category::CompileTime,
                        "invalid compile-time integer argument",
                    )
                    .with_primary(span)
                }),
            _ => Err(Diagnostic::new(
                Category::CompileTime,
                "this compile-time argument must be an integer literal",
            )
            .with_primary(span)),
        },
        _ => Err(Diagnostic::new(
            Category::CompileTime,
            "this compile-time parameter type cannot be supplied as syntax",
        )
        .with_primary(span)),
    }
}

fn parse_tokens(tokens: &[Token], kind: FragmentKind, span: Span) -> crate::parser::ParseOutput {
    let mut tokens = tokens.to_vec();
    let boundary = tokens.last().map_or(span, |token| token.span);
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(boundary.file, boundary.end, boundary.end),
    });
    parse_fragment(&tokens, kind)
}

fn split_arguments(tokens: &[Token]) -> Vec<Vec<Token>> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1)
            }
            TokenKind::Comma if depth == 0 => {
                result.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(token.clone());
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn expansion_role(ty: &CompileTimeType) -> Option<ExpansionRole> {
    Some(match ty {
        CompileTimeType::Ast(QuoteRole::Expression) => ExpansionRole::Expression,
        CompileTimeType::Ast(QuoteRole::Pattern) => ExpansionRole::Pattern,
        CompileTimeType::Ast(QuoteRole::TypeSyntax) => ExpansionRole::Type,
        CompileTimeType::Ast(QuoteRole::StatementList) => ExpansionRole::Statements,
        CompileTimeType::Ast(QuoteRole::Item | QuoteRole::ItemList) => ExpansionRole::Items,
        _ => return None,
    })
}

fn child_position(parent: SyntaxKind) -> ExpansionRole {
    if matches!(
        parent,
        SyntaxKind::Pattern
            | SyntaxKind::AlternativePattern
            | SyntaxKind::DereferencePattern
            | SyntaxKind::TuplePattern
            | SyntaxKind::RecordPattern
            | SyntaxKind::VariantPattern
            | SyntaxKind::PatternField
    ) {
        ExpansionRole::Pattern
    } else if matches!(
        parent,
        SyntaxKind::Type
            | SyntaxKind::TypeArguments
            | SyntaxKind::TypeAlias
            | SyntaxKind::Field
            | SyntaxKind::Parameter
            | SyntaxKind::Function
            | SyntaxKind::Impl
            | SyntaxKind::GenericParameter
    ) {
        ExpansionRole::Type
    } else {
        ExpansionRole::Expression
    }
}

fn definition_role(kind: SyntaxKind) -> Option<QuoteRole> {
    Some(match kind {
        SyntaxKind::Struct => QuoteRole::StructDefinition,
        SyntaxKind::Enum => QuoteRole::EnumDefinition,
        SyntaxKind::Function => QuoteRole::FunctionDefinition,
        _ => return None,
    })
}

fn derive_output_matches(
    implementation: &SyntaxNode,
    expected_trait: &[String],
    target: &SyntaxNode,
) -> bool {
    let types = implementation.direct_children(SyntaxKind::Type);
    let output_trait = types.first().map(|node| written_type_path(node));
    let expected_target = declaration_name(target);
    let output_target = types.get(1).map(|node| written_type_path(node));
    paths_name_same_identity(output_trait.as_deref(), Some(expected_trait))
        && expected_target.as_ref().is_some_and(|expected| {
            paths_name_same_identity(
                output_target.as_deref(),
                Some(std::slice::from_ref(expected)),
            )
        })
}

fn written_type_path(node: &SyntaxNode) -> Vec<String> {
    node.direct_tokens()
        .into_iter()
        .filter_map(|token| path_component(&token.kind))
        .collect()
}

fn paths_name_same_identity(output: Option<&[String]>, expected: Option<&[String]>) -> bool {
    let (Some(output), Some(expected)) = (output, expected) else {
        return false;
    };
    if output.len() == 1 && expected.len() == 1 {
        output[0] == expected[0]
    } else {
        output == expected
    }
}

fn attachment_kind(node: &SyntaxNode) -> Option<CompileTimeNamespace> {
    node.direct_tokens()
        .into_iter()
        .find_map(|token| match token.kind {
            TokenKind::Keyword(Keyword::Attr) => Some(CompileTimeNamespace::Attribute),
            TokenKind::Keyword(Keyword::Derive) => Some(CompileTimeNamespace::Derive),
            _ => None,
        })
}

fn derive_paths(node: &SyntaxNode) -> Vec<Vec<String>> {
    let tokens = node.direct_tokens();
    let start = tokens
        .iter()
        .position(|token| token.kind == TokenKind::LParen)
        .map_or(0, |index| index + 1);
    let end = tokens
        .iter()
        .rposition(|token| token.kind == TokenKind::RParen)
        .unwrap_or(tokens.len());
    split_arguments(
        &tokens[start..end]
            .iter()
            .map(|token| (*token).clone())
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .map(|tokens| {
        tokens
            .iter()
            .filter_map(|token| path_component(&token.kind))
            .collect()
    })
    .collect()
}

fn path_tokens(node: &SyntaxNode, path: &[String]) -> Vec<Token> {
    let mut wanted = path.iter();
    let mut next = wanted.next();
    node.direct_tokens()
        .into_iter()
        .filter_map(|token| {
            if path_component(&token.kind).as_ref() == next {
                next = wanted.next();
                Some(token.clone())
            } else {
                None
            }
        })
        .collect()
}

fn strip_custom_attachments(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    node.children.retain(|child| {
        !matches!(
            child,
            SyntaxElement::Node(attribute)
                if attribute.kind == SyntaxKind::Attribute
                    && attachment_kind(attribute).is_some()
        )
    });
    node
}

fn attach_builtin_derives(node: &SyntaxNode, tokens: &[Token]) -> SyntaxNode {
    if tokens.is_empty() || !matches!(node.kind, SyntaxKind::Struct | SyntaxKind::Enum) {
        return node.clone();
    }
    let mut node = node.clone();
    let mut children = Vec::new();
    let mut first = true;
    for token in tokens {
        if !first {
            children.push(SyntaxElement::Token(Token {
                kind: TokenKind::Comma,
                span: token.span,
            }));
        }
        children.push(SyntaxElement::Token(token.clone()));
        first = false;
    }
    node.children
        .push(SyntaxElement::Node(Box::new(SyntaxNode::new(
            SyntaxKind::DeriveList,
            children,
            node.span,
        ))));
    node
}

fn is_user_macro(node: &SyntaxNode) -> bool {
    macro_call(node)
        .and_then(|call| call.path.first().cloned())
        .is_some_and(|name| !matches!(name.as_str(), "vec" | "map" | "set"))
}

fn is_definition(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::Struct | SyntaxKind::Enum | SyntaxKind::Function
    )
}

fn contains_generated_compile_time_declaration(node: &SyntaxNode) -> bool {
    matches!(
        node.kind,
        SyntaxKind::MacroDeclaration
            | SyntaxKind::AttributeDeclaration
            | SyntaxKind::DeriveDeclaration
    ) || (node.kind == SyntaxKind::Use
        && node.direct_tokens().iter().any(|token| {
            matches!(
                token.kind,
                TokenKind::Keyword(Keyword::Macro | Keyword::Attr | Keyword::Derive)
            )
        }))
        || node
            .direct_nodes()
            .into_iter()
            .any(contains_generated_compile_time_declaration)
}

fn declaration_name(node: &SyntaxNode) -> Option<String> {
    let mut saw = false;
    node.direct_tokens()
        .into_iter()
        .find_map(|token| match &token.kind {
            TokenKind::Keyword(Keyword::Mod | Keyword::Struct | Keyword::Enum | Keyword::Fn) => {
                saw = true;
                None
            }
            TokenKind::Identifier(name) if saw => Some(name.clone()),
            _ => None,
        })
}

fn path_component(kind: &TokenKind) -> Option<String> {
    match kind {
        TokenKind::Identifier(name) => Some(name.clone()),
        TokenKind::Keyword(Keyword::Root) => Some("root".to_string()),
        TokenKind::Keyword(Keyword::SelfValue) => Some("self".to_string()),
        TokenKind::Keyword(Keyword::Super) => Some("super".to_string()),
        _ => None,
    }
}

fn recovery_node(span: Span) -> SyntaxNode {
    SyntaxNode {
        kind: SyntaxKind::Error,
        span,
        children: Vec::new(),
    }
}

fn structural_text(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|token| token_source(&token.kind))
        .collect::<Vec<_>>()
        .join(" ")
}

fn count_nodes(node: &SyntaxNode) -> u64 {
    1 + node
        .direct_nodes()
        .into_iter()
        .map(count_nodes)
        .sum::<u64>()
}

fn all_tokens(node: &SyntaxNode) -> Vec<&Token> {
    fn walk<'a>(node: &'a SyntaxNode, output: &mut Vec<&'a Token>) {
        for child in &node.children {
            match child {
                SyntaxElement::Token(token) => output.push(token),
                SyntaxElement::Node(node) => walk(node, output),
            }
        }
    }
    let mut output = Vec::new();
    walk(node, &mut output);
    output
}

fn token_source(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Identifier(name) => name.clone(),
        TokenKind::StringLiteral(value) => value.clone(),
        TokenKind::IntegerLiteral { raw, .. } | TokenKind::FloatLiteral { raw, .. } => raw.clone(),
        _ => format!("{kind:?}"),
    }
}

fn unit_package_path(identity: &ExpandedUnitIdentity) -> Option<(PackageId, Vec<String>)> {
    match identity {
        ExpandedUnitIdentity::Standard(_) => None,
        ExpandedUnitIdentity::PackageRoot(package) => Some((package.clone(), Vec::new())),
        ExpandedUnitIdentity::PackageModule { package, path } => {
            Some((package.clone(), path.components().to_vec()))
        }
    }
}
