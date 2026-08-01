//! Deterministic fixed-point scheduling for compile-time syntax generation.
//!
//! This module deliberately does not execute user code. It owns the ordering,
//! dependency, generated-output re-entry, active-chain cycle, and recovery
//! contracts that a later compile-time interpreter drives.

use std::collections::BTreeSet;

use crate::expansion::namespace::CompileTimeDeclarationId;
use crate::expansion::provenance::OriginId;
use crate::expansion::token_tree::{TokenTree, flatten_token_trees};
use crate::package::PackageId;
use crate::syntax::TokenKind;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            #[must_use]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

identity!(ExpansionWorkId);
identity!(RecoveryNodeId);

/// Normative compile-time resource limits from `SPEC.md` §12.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpansionLimits {
    pub active_depth: u32,
    pub executions: u64,
    pub generated_nodes: u64,
    pub interpreter_steps: u64,
    pub live_value_bytes: u64,
}

impl Default for ExpansionLimits {
    fn default() -> Self {
        Self {
            active_depth: 128,
            executions: 65_536,
            generated_nodes: 1_048_576,
            interpreter_steps: 1_048_576,
            live_value_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimitKind {
    ActiveDepth,
    Executions,
    GeneratedNodes,
    InterpreterSteps,
    LiveValueBytes,
}

/// Shared graph-wide consumption charged in deterministic scheduler order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpansionResourceUsage {
    pub executions: u64,
    pub generated_nodes: u64,
    pub maximum_depth: u32,
}

/// Final per-execution meter values retained with scheduled work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutionResourceUsage {
    pub interpreter_steps: u64,
    pub live_value_bytes: u64,
    pub peak_live_value_bytes: u64,
}

/// Sticky per-execution fuel and live-value meter supplied to the interpreter.
/// Ignoring a failed charge cannot admit output: [`ExpansionScheduler::run`]
/// checks the sticky exhaustion state after the callback returns.
#[derive(Debug)]
pub struct ExecutionResources {
    step_limit: u64,
    live_value_limit: u64,
    usage: ExecutionResourceUsage,
    exhausted: Option<ResourceLimitKind>,
}

impl ExecutionResources {
    pub(crate) fn new(limits: ExpansionLimits) -> Self {
        Self {
            step_limit: limits.interpreter_steps,
            live_value_limit: limits.live_value_bytes,
            usage: ExecutionResourceUsage::default(),
            exhausted: None,
        }
    }

    pub fn charge_steps(&mut self, steps: u64) -> Result<(), ResourceLimitKind> {
        let Some(total) = self.usage.interpreter_steps.checked_add(steps) else {
            return self.exhaust(ResourceLimitKind::InterpreterSteps);
        };
        if total > self.step_limit {
            return self.exhaust(ResourceLimitKind::InterpreterSteps);
        }
        self.usage.interpreter_steps = total;
        Ok(())
    }

    pub fn allocate_live_value_bytes(&mut self, bytes: u64) -> Result<(), ResourceLimitKind> {
        let Some(total) = self.usage.live_value_bytes.checked_add(bytes) else {
            return self.exhaust(ResourceLimitKind::LiveValueBytes);
        };
        if total > self.live_value_limit {
            return self.exhaust(ResourceLimitKind::LiveValueBytes);
        }
        self.usage.live_value_bytes = total;
        self.usage.peak_live_value_bytes = self.usage.peak_live_value_bytes.max(total);
        Ok(())
    }

    pub fn release_live_value_bytes(&mut self, bytes: u64) {
        self.usage.live_value_bytes = self
            .usage
            .live_value_bytes
            .checked_sub(bytes)
            .expect("the interpreter cannot release more live memory than it owns");
    }

    #[must_use]
    pub fn usage(&self) -> ExecutionResourceUsage {
        self.usage
    }

    fn exhaust(&mut self, limit: ResourceLimitKind) -> Result<(), ResourceLimitKind> {
        self.exhausted.get_or_insert(limit);
        Err(limit)
    }
}

/// The syntactic result role of one compile-time execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExpansionRole {
    Attribute,
    Derive,
    Expression,
    Pattern,
    Type,
    Statements,
    Items,
}

/// One origin-independent token in a structural cycle key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralToken {
    pub kind: TokenKind,
    pub source: String,
}

/// Origin-independent syntax used by active-chain cycle detection.
///
/// Physical and generated occurrences compare equally when their represented
/// token kinds and spellings are equal. Layout and delimiter tokens remain in
/// the sequence, so this is structural source input rather than a text hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructuralInput {
    pub tokens: Vec<StructuralToken>,
}

impl StructuralInput {
    #[must_use]
    pub fn from_token_trees(trees: &[TokenTree]) -> Self {
        Self {
            tokens: flatten_token_trees(trees)
                .into_iter()
                .map(|token| StructuralToken {
                    kind: token.kind.clone(),
                    source: token.source.clone(),
                })
                .collect(),
        }
    }
}

/// Stable global ordering location for one expansion request.
///
/// `provenance_order` starts with a physical source offset. Generated work
/// appends deterministic output indices, preserving parent-before-child and
/// left-to-right output order without assigning generated syntax a fake span.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExpansionLocation {
    pub package: PackageId,
    pub module: Vec<String>,
    pub provenance_order: Vec<u32>,
}

impl ExpansionLocation {
    #[must_use]
    pub fn physical(package: PackageId, module: Vec<String>, source_offset: u32) -> Self {
        Self {
            package,
            module,
            provenance_order: vec![source_offset],
        }
    }

    #[must_use]
    pub fn generated(&self, output_index: u32) -> Self {
        let mut location = self.clone();
        location.provenance_order.push(output_index);
        location
    }
}

/// Everything known before one compile-time body is executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionRequest {
    pub declaration: CompileTimeDeclarationId,
    pub role: ExpansionRole,
    pub input: StructuralInput,
    pub location: ExpansionLocation,
    pub invocation: OriginId,
    pub definition: OriginId,
}

impl ExpansionRequest {
    fn repeats(&self, other: &Self) -> bool {
        self.declaration == other.declaration
            && self.role == other.role
            && self.input == other.input
    }
}

/// Expansion work attached to one definition, in source order within a stage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefinitionExpansions {
    pub attributes: Vec<ExpansionRequest>,
    pub derives: Vec<ExpansionRequest>,
    /// Outermost function-like invocations visible after attachments finish.
    pub invocations: Vec<ExpansionRequest>,
}

/// New structural work returned by one successful execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduledOutput {
    /// Structurally counted AST nodes in the complete, validated output.
    /// Charging occurs atomically before any contained work is admitted.
    pub generated_nodes: u64,
    pub invocations: Vec<ExpansionRequest>,
    pub definitions: Vec<DefinitionExpansions>,
}

/// Why one scheduled request became an explicit recovery node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    Cycle,
    Dependency,
    InvalidOutput,
    ExecutionFailure,
    ResourceLimit(ResourceLimitKind),
}

/// One stable placeholder that lets independent work continue after failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryNode {
    pub id: RecoveryNodeId,
    pub work: ExpansionWorkId,
    pub role: ExpansionRole,
    pub reason: RecoveryReason,
    /// Matching ancestor through the recovered request, for a cycle.
    pub active_chain: Vec<ExpansionWorkId>,
}

/// An origin-aware scheduling problem retained for later diagnostic rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleProblem {
    pub work: ExpansionWorkId,
    pub invocation: OriginId,
    pub definition: OriginId,
    pub reason: RecoveryReason,
    pub active_chain: Vec<ExpansionWorkId>,
}

impl ScheduleProblem {
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self.reason {
            RecoveryReason::Cycle => {
                "compile-time expansion re-entered the same declaration, role, and input"
            }
            RecoveryReason::Dependency => "compile-time expansion dependency did not complete",
            RecoveryReason::InvalidOutput => {
                "compile-time expansion returned structurally invalid output"
            }
            RecoveryReason::ExecutionFailure => "compile-time expansion execution failed",
            RecoveryReason::ResourceLimit(_) => "compile-time expansion exceeded a resource limit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpansionWorkState {
    Pending,
    Ready,
    Active,
    Completed,
    Recovered(RecoveryNodeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Stage,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpansionDependency {
    pub prerequisite: ExpansionWorkId,
    pub dependent: ExpansionWorkId,
    pub kind: DependencyKind,
}

/// One request and its stable scheduling facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledExpansion {
    pub id: ExpansionWorkId,
    pub request: ExpansionRequest,
    pub parent: Option<ExpansionWorkId>,
    pub dependencies: Vec<ExpansionWorkId>,
    pub depth: u32,
    pub resources: Option<ExecutionResourceUsage>,
    pub state: ExpansionWorkState,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReadyKey {
    location: ExpansionLocation,
    work: ExpansionWorkId,
}

/// Append-only dependency graph plus a deterministically ordered ready set.
#[derive(Debug, Default)]
pub struct ExpansionScheduler {
    work: Vec<ScheduledExpansion>,
    edges: Vec<ExpansionDependency>,
    recoveries: Vec<RecoveryNode>,
    problems: Vec<ScheduleProblem>,
    ready: BTreeSet<ReadyKey>,
    limits: ExpansionLimits,
    usage: ExpansionResourceUsage,
}

impl ExpansionScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_limits(limits: ExpansionLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn limits(&self) -> ExpansionLimits {
        self.limits
    }

    #[must_use]
    pub fn usage(&self) -> ExpansionResourceUsage {
        self.usage
    }

    #[must_use]
    pub fn work(&self) -> &[ScheduledExpansion] {
        &self.work
    }

    #[must_use]
    pub fn dependencies(&self) -> &[ExpansionDependency] {
        &self.edges
    }

    #[must_use]
    pub fn recoveries(&self) -> &[RecoveryNode] {
        &self.recoveries
    }

    #[must_use]
    pub fn problems(&self) -> &[ScheduleProblem] {
        &self.problems
    }

    pub fn enqueue(&mut self, request: ExpansionRequest) -> ExpansionWorkId {
        self.enqueue_internal(request, None, Vec::new(), DependencyKind::Stage)
    }

    /// Enqueues attributes, then derives, then visible outermost invocations.
    /// Each request depends on the preceding stage, enforcing the normative
    /// order even when locations from unrelated definitions interleave.
    pub fn enqueue_definition(&mut self, definition: DefinitionExpansions) -> Vec<ExpansionWorkId> {
        self.enqueue_definition_after(None, definition)
    }

    /// Selects the next ready execution by package, module, provenance, then
    /// stable work identity.
    fn next_ready(&mut self) -> Option<ExpansionWorkId> {
        loop {
            let key = self.ready.pop_first()?;
            let record = &mut self.work[key.work.index()];
            assert_eq!(
                record.state,
                ExpansionWorkState::Ready,
                "only ready work may occur in the ready set"
            );
            if self.usage.executions >= self.limits.executions {
                self.recover(
                    key.work,
                    RecoveryReason::ResourceLimit(ResourceLimitKind::Executions),
                    Vec::new(),
                );
                continue;
            }
            self.usage.executions += 1;
            self.work[key.work.index()].state = ExpansionWorkState::Active;
            return Some(key.work);
        }
    }

    /// Completes an active execution and re-enters all generated work.
    fn complete(&mut self, id: ExpansionWorkId, output: ScheduledOutput) -> Vec<ExpansionWorkId> {
        self.assert_active(id);
        let Some(generated_nodes) = self
            .usage
            .generated_nodes
            .checked_add(output.generated_nodes)
        else {
            self.recover(
                id,
                RecoveryReason::ResourceLimit(ResourceLimitKind::GeneratedNodes),
                Vec::new(),
            );
            return Vec::new();
        };
        if generated_nodes > self.limits.generated_nodes {
            self.recover(
                id,
                RecoveryReason::ResourceLimit(ResourceLimitKind::GeneratedNodes),
                Vec::new(),
            );
            return Vec::new();
        }
        self.usage.generated_nodes = generated_nodes;
        self.work[id.index()].state = ExpansionWorkState::Completed;
        self.refresh_dependents(id);

        let mut added = Vec::new();
        for request in output.invocations {
            added.push(self.enqueue_internal(
                request,
                Some(id),
                vec![id],
                DependencyKind::Generated,
            ));
        }
        for definition in output.definitions {
            added.extend(self.enqueue_definition_after(Some(id), definition));
        }
        added
    }

    /// Recovers one active execution without admitting partial output.
    fn fail(&mut self, id: ExpansionWorkId, reason: RecoveryReason) {
        assert!(!matches!(
            reason,
            RecoveryReason::Cycle | RecoveryReason::Dependency
        ));
        self.assert_active(id);
        self.recover(id, reason, Vec::new());
    }

    /// Drives the scheduler to a fixed point with a future executor-shaped
    /// callback. The callback is deliberately unaware of scheduler internals.
    pub fn run(
        &mut self,
        mut execute: impl FnMut(
            &ScheduledExpansion,
            &mut ExecutionResources,
        ) -> Result<ScheduledOutput, RecoveryReason>,
    ) {
        while let Some(id) = self.next_ready() {
            let record = self.work[id.index()].clone();
            let mut resources = ExecutionResources::new(self.limits);
            let result = execute(&record, &mut resources);
            self.work[id.index()].resources = Some(resources.usage());
            if let Some(limit) = resources.exhausted {
                self.fail(id, RecoveryReason::ResourceLimit(limit));
                continue;
            }
            match result {
                Ok(output) => {
                    self.complete(id, output);
                }
                Err(reason) => self.fail(id, reason),
            }
        }
    }

    fn enqueue_definition_after(
        &mut self,
        generated_by: Option<ExpansionWorkId>,
        definition: DefinitionExpansions,
    ) -> Vec<ExpansionWorkId> {
        for request in &definition.attributes {
            assert_eq!(request.role, ExpansionRole::Attribute);
        }
        for request in &definition.derives {
            assert_eq!(request.role, ExpansionRole::Derive);
        }

        let mut added = Vec::new();
        let mut previous = generated_by;
        for request in definition
            .attributes
            .into_iter()
            .chain(definition.derives)
            .chain(definition.invocations)
        {
            let dependencies = previous.into_iter().collect::<Vec<_>>();
            let kind = if previous == generated_by && generated_by.is_some() {
                DependencyKind::Generated
            } else {
                DependencyKind::Stage
            };
            let id = self.enqueue_internal(request, previous, dependencies, kind);
            previous = Some(id);
            added.push(id);
        }
        added
    }

    fn enqueue_internal(
        &mut self,
        request: ExpansionRequest,
        parent: Option<ExpansionWorkId>,
        dependencies: Vec<ExpansionWorkId>,
        dependency_kind: DependencyKind,
    ) -> ExpansionWorkId {
        for dependency in &dependencies {
            assert!(
                dependency.index() < self.work.len(),
                "dependencies must already have stable work identities"
            );
        }
        let id = ExpansionWorkId(
            u32::try_from(self.work.len()).expect("more than u32::MAX scheduled expansions"),
        );
        let depth = parent.map_or(1, |parent| {
            self.work[parent.index()]
                .depth
                .checked_add(1)
                .expect("more than u32::MAX nested expansions")
        });
        self.work.push(ScheduledExpansion {
            id,
            request,
            parent,
            dependencies: dependencies.clone(),
            depth,
            resources: None,
            state: ExpansionWorkState::Pending,
        });
        for prerequisite in dependencies {
            self.edges.push(ExpansionDependency {
                prerequisite,
                dependent: id,
                kind: dependency_kind,
            });
        }

        self.usage.maximum_depth = self.usage.maximum_depth.max(depth);
        if depth > self.limits.active_depth {
            self.recover(
                id,
                RecoveryReason::ResourceLimit(ResourceLimitKind::ActiveDepth),
                Vec::new(),
            );
        } else if let Some(active_chain) = self.cycle_chain(id) {
            self.recover(id, RecoveryReason::Cycle, active_chain);
        } else {
            self.refresh(id);
        }
        id
    }

    fn cycle_chain(&self, candidate: ExpansionWorkId) -> Option<Vec<ExpansionWorkId>> {
        let request = &self.work[candidate.index()].request;
        let mut reverse_chain = vec![candidate];
        let mut cursor = self.work[candidate.index()].parent;
        while let Some(ancestor) = cursor {
            reverse_chain.push(ancestor);
            if request.repeats(&self.work[ancestor.index()].request) {
                reverse_chain.reverse();
                return Some(reverse_chain);
            }
            cursor = self.work[ancestor.index()].parent;
        }
        None
    }

    fn refresh(&mut self, id: ExpansionWorkId) {
        if self.work[id.index()].state != ExpansionWorkState::Pending {
            return;
        }
        let dependencies = self.work[id.index()].dependencies.clone();
        if dependencies.iter().any(|dependency| {
            matches!(
                self.work[dependency.index()].state,
                ExpansionWorkState::Recovered(_)
            )
        }) {
            self.recover(id, RecoveryReason::Dependency, Vec::new());
            return;
        }
        if dependencies
            .iter()
            .all(|dependency| self.work[dependency.index()].state == ExpansionWorkState::Completed)
        {
            self.work[id.index()].state = ExpansionWorkState::Ready;
            self.ready.insert(ReadyKey {
                location: self.work[id.index()].request.location.clone(),
                work: id,
            });
        }
    }

    fn refresh_dependents(&mut self, prerequisite: ExpansionWorkId) {
        let mut dependents = self
            .edges
            .iter()
            .filter_map(|edge| (edge.prerequisite == prerequisite).then_some(edge.dependent))
            .collect::<Vec<_>>();
        dependents.sort_by(|left, right| {
            self.work[left.index()]
                .request
                .location
                .cmp(&self.work[right.index()].request.location)
                .then_with(|| left.cmp(right))
        });
        dependents.dedup();
        for dependent in dependents {
            self.refresh(dependent);
        }
    }

    fn recover(
        &mut self,
        work: ExpansionWorkId,
        reason: RecoveryReason,
        active_chain: Vec<ExpansionWorkId>,
    ) {
        assert!(
            !matches!(
                self.work[work.index()].state,
                ExpansionWorkState::Completed | ExpansionWorkState::Recovered(_)
            ),
            "completed work cannot be recovered"
        );
        self.ready.remove(&ReadyKey {
            location: self.work[work.index()].request.location.clone(),
            work,
        });
        let recovery = RecoveryNodeId(
            u32::try_from(self.recoveries.len()).expect("more than u32::MAX recovery nodes"),
        );
        let request = &self.work[work.index()].request;
        self.recoveries.push(RecoveryNode {
            id: recovery,
            work,
            role: request.role,
            reason,
            active_chain: active_chain.clone(),
        });
        if reason != RecoveryReason::Dependency {
            self.problems.push(ScheduleProblem {
                work,
                invocation: request.invocation,
                definition: request.definition,
                reason,
                active_chain,
            });
        }
        self.work[work.index()].state = ExpansionWorkState::Recovered(recovery);
        self.refresh_dependents(work);
    }

    fn assert_active(&self, id: ExpansionWorkId) {
        assert_eq!(
            self.work[id.index()].state,
            ExpansionWorkState::Active,
            "only active work may finish"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::expansion::provenance::ProvenanceTable;
    use crate::package::PackageGraph;
    use crate::source::{SourceManager, Span};
    use proptest::prelude::*;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        package: PackageId,
        invocation: OriginId,
        definition: OriginId,
        path: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "elamite-scheduler-{}-{serial}.elx",
                std::process::id()
            ));
            fs::write(&path, " ".repeat(32)).expect("write scheduler fixture");
            let graph = PackageGraph::single_file(&path).expect("make implicit package");
            let mut sources = SourceManager::new();
            let file = sources.add_text(path.clone(), " ".repeat(32));
            let mut provenance = ProvenanceTable::new();
            let definition = provenance.register_physical(Span::new(file, 0, 1));
            let invocation = provenance.register_physical(Span::new(file, 16, 17));
            Self {
                package: graph.root,
                invocation,
                definition,
                path,
            }
        }

        fn request(
            &self,
            declaration: u32,
            role: ExpansionRole,
            input: &str,
            module: &[&str],
            order: &[u32],
        ) -> ExpansionRequest {
            ExpansionRequest {
                declaration: CompileTimeDeclarationId::from_index(declaration),
                role,
                input: StructuralInput {
                    tokens: vec![StructuralToken {
                        kind: TokenKind::Identifier(input.to_string()),
                        source: input.to_string(),
                    }],
                },
                location: ExpansionLocation {
                    package: self.package.clone(),
                    module: module.iter().map(|part| (*part).to_string()).collect(),
                    provenance_order: order.to_vec(),
                },
                invocation: self.invocation,
                definition: self.definition,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn ready_work_is_ordered_by_module_and_provenance_not_insertion() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::new();
        let late =
            scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "late", &["z"], &[9]));
        let first =
            scheduler.enqueue(fixture.request(1, ExpansionRole::Expression, "first", &["a"], &[8]));
        let second = scheduler.enqueue(fixture.request(
            2,
            ExpansionRole::Expression,
            "second",
            &["a"],
            &[12],
        ));

        assert_eq!(scheduler.next_ready(), Some(first));
        scheduler.complete(first, ScheduledOutput::default());
        assert_eq!(scheduler.next_ready(), Some(second));
        scheduler.complete(second, ScheduledOutput::default());
        assert_eq!(scheduler.next_ready(), Some(late));
    }

    #[test]
    fn definition_stages_attributes_then_derives_then_invocations() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::new();
        let ids = scheduler.enqueue_definition(DefinitionExpansions {
            attributes: vec![
                fixture.request(0, ExpansionRole::Attribute, "a", &[], &[0]),
                fixture.request(1, ExpansionRole::Attribute, "b", &[], &[1]),
            ],
            derives: vec![
                fixture.request(2, ExpansionRole::Derive, "c", &[], &[2]),
                fixture.request(3, ExpansionRole::Derive, "d", &[], &[3]),
            ],
            invocations: vec![fixture.request(4, ExpansionRole::Expression, "e", &[], &[4])],
        });
        let mut visited = Vec::new();
        scheduler.run(|work, _| {
            visited.push(work.id);
            Ok(ScheduledOutput::default())
        });

        assert_eq!(visited, ids);
        assert_eq!(scheduler.dependencies().len(), 4);
        assert!(
            scheduler
                .work()
                .iter()
                .all(|work| work.state == ExpansionWorkState::Completed)
        );
    }

    #[test]
    fn generated_invocations_and_definitions_reenter_the_same_queue() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::new();
        scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "outer", &[], &[10]));
        scheduler.enqueue(fixture.request(5, ExpansionRole::Expression, "sibling", &[], &[20]));
        let mut visited = Vec::new();
        scheduler.run(|work, _| {
            visited.push(work.request.declaration.index());
            if work.request.declaration.index() == 0 {
                Ok(ScheduledOutput {
                    generated_nodes: 4,
                    invocations: vec![
                        fixture.request(1, ExpansionRole::Expression, "left", &[], &[10, 0]),
                        fixture.request(2, ExpansionRole::Expression, "right", &[], &[10, 1]),
                    ],
                    definitions: vec![DefinitionExpansions {
                        attributes: vec![fixture.request(
                            3,
                            ExpansionRole::Attribute,
                            "generated-attribute",
                            &[],
                            &[10, 2],
                        )],
                        derives: vec![fixture.request(
                            4,
                            ExpansionRole::Derive,
                            "generated-derive",
                            &[],
                            &[10, 3],
                        )],
                        invocations: Vec::new(),
                    }],
                })
            } else {
                Ok(ScheduledOutput::default())
            }
        });

        assert_eq!(visited, [0, 1, 2, 3, 4, 5]);
        assert_eq!(scheduler.work().len(), 6);
        assert!(scheduler.dependencies().iter().any(|edge| {
            edge.kind == DependencyKind::Generated
                && edge.prerequisite == ExpansionWorkId(0)
                && edge.dependent == ExpansionWorkId(2)
        }));
    }

    #[test]
    fn equal_active_chain_input_recovers_as_a_cycle_but_changed_input_is_allowed() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::new();
        scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "same", &[], &[0]));
        scheduler.run(|work, _| {
            let generated = fixture.request(
                0,
                ExpansionRole::Expression,
                "changed",
                &[],
                &work.request.location.generated(0).provenance_order,
            );
            Ok(ScheduledOutput {
                generated_nodes: 1,
                invocations: vec![generated],
                definitions: Vec::new(),
            })
        });

        assert_eq!(scheduler.work().len(), 3);
        assert_eq!(scheduler.recoveries().len(), 1);
        let recovery = &scheduler.recoveries()[0];
        assert_eq!(recovery.reason, RecoveryReason::Cycle);
        assert_eq!(recovery.active_chain.len(), 2);
        assert_eq!(scheduler.problems()[0].active_chain, recovery.active_chain);
    }

    #[test]
    fn failed_stage_recovers_dependents_and_leaves_independent_work_ready() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::new();
        let staged = scheduler.enqueue_definition(DefinitionExpansions {
            attributes: vec![fixture.request(0, ExpansionRole::Attribute, "fails", &[], &[0])],
            derives: vec![fixture.request(1, ExpansionRole::Derive, "blocked", &[], &[1])],
            invocations: Vec::new(),
        });
        let independent =
            scheduler.enqueue(fixture.request(2, ExpansionRole::Items, "independent", &[], &[2]));

        assert_eq!(scheduler.next_ready(), Some(staged[0]));
        scheduler.fail(staged[0], RecoveryReason::InvalidOutput);
        assert!(matches!(
            scheduler.work()[staged[1].index()].state,
            ExpansionWorkState::Recovered(_)
        ));
        assert_eq!(scheduler.next_ready(), Some(independent));
    }

    #[test]
    fn default_limits_match_the_normative_compile_time_contract() {
        assert_eq!(
            ExpansionLimits::default(),
            ExpansionLimits {
                active_depth: 128,
                executions: 65_536,
                generated_nodes: 1_048_576,
                interpreter_steps: 1_048_576,
                live_value_bytes: 64 * 1024 * 1024,
            }
        );
    }

    #[test]
    fn execution_budget_is_charged_in_ready_order() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::with_limits(ExpansionLimits {
            executions: 2,
            ..ExpansionLimits::default()
        });
        for (declaration, order) in [(0, 2), (1, 0), (2, 1)] {
            scheduler.enqueue(fixture.request(
                declaration,
                ExpansionRole::Expression,
                "input",
                &[],
                &[order],
            ));
        }
        let mut visited = Vec::new();
        scheduler.run(|work, _| {
            visited.push(work.request.declaration.index());
            Ok(ScheduledOutput::default())
        });

        assert_eq!(visited, [1, 2]);
        assert_eq!(scheduler.usage().executions, 2);
        assert_eq!(scheduler.problems().len(), 1);
        assert_eq!(
            scheduler.problems()[0].reason,
            RecoveryReason::ResourceLimit(ResourceLimitKind::Executions)
        );
    }

    #[test]
    fn generated_node_charging_is_atomic_before_output_reentry() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::with_limits(ExpansionLimits {
            generated_nodes: 2,
            ..ExpansionLimits::default()
        });
        scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "oversized", &[], &[0]));
        scheduler.enqueue(fixture.request(1, ExpansionRole::Expression, "independent", &[], &[1]));
        scheduler.run(|work, _| {
            if work.request.declaration.index() == 0 {
                Ok(ScheduledOutput {
                    generated_nodes: 3,
                    invocations: vec![fixture.request(
                        2,
                        ExpansionRole::Expression,
                        "must-not-enter",
                        &[],
                        &[0, 0],
                    )],
                    definitions: Vec::new(),
                })
            } else {
                Ok(ScheduledOutput {
                    generated_nodes: 1,
                    ..ScheduledOutput::default()
                })
            }
        });

        assert_eq!(scheduler.work().len(), 2);
        assert_eq!(scheduler.usage().generated_nodes, 1);
        assert_eq!(
            scheduler.problems()[0].reason,
            RecoveryReason::ResourceLimit(ResourceLimitKind::GeneratedNodes)
        );
        assert_eq!(scheduler.work()[1].state, ExpansionWorkState::Completed);
    }

    #[test]
    fn generated_work_cannot_bypass_the_active_depth_limit() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::with_limits(ExpansionLimits {
            active_depth: 2,
            ..ExpansionLimits::default()
        });
        scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "root", &[], &[0]));
        let mut visited = Vec::new();
        scheduler.run(|work, _| {
            let declaration = work.request.declaration.index();
            visited.push(declaration);
            Ok(ScheduledOutput {
                generated_nodes: 1,
                invocations: vec![fixture.request(
                    u32::try_from(declaration + 1).expect("small test identity"),
                    ExpansionRole::Expression,
                    "changing-input",
                    &[],
                    &work.request.location.generated(0).provenance_order,
                )],
                definitions: Vec::new(),
            })
        });

        assert_eq!(visited, [0, 1]);
        assert_eq!(scheduler.work().len(), 3);
        assert_eq!(scheduler.usage().maximum_depth, 3);
        assert_eq!(
            scheduler.problems()[0].reason,
            RecoveryReason::ResourceLimit(ResourceLimitKind::ActiveDepth)
        );
    }

    #[test]
    fn ignored_interpreter_meter_failures_still_discard_output() {
        let fixture = Fixture::new();
        let mut scheduler = ExpansionScheduler::with_limits(ExpansionLimits {
            interpreter_steps: 2,
            live_value_bytes: 4,
            ..ExpansionLimits::default()
        });
        scheduler.enqueue(fixture.request(0, ExpansionRole::Expression, "fuel", &[], &[0]));
        scheduler.enqueue(fixture.request(1, ExpansionRole::Expression, "memory", &[], &[1]));
        scheduler.run(|work, resources| {
            if work.request.declaration.index() == 0 {
                let _ = resources.charge_steps(3);
            } else {
                resources
                    .allocate_live_value_bytes(4)
                    .expect("allocation reaches the limit");
                resources.release_live_value_bytes(3);
                resources
                    .allocate_live_value_bytes(3)
                    .expect("released storage can be reused");
                let _ = resources.allocate_live_value_bytes(1);
            }
            Ok(ScheduledOutput {
                generated_nodes: 1,
                invocations: vec![fixture.request(
                    2,
                    ExpansionRole::Expression,
                    "must-not-enter",
                    &[],
                    &work.request.location.generated(0).provenance_order,
                )],
                definitions: Vec::new(),
            })
        });

        assert_eq!(scheduler.work().len(), 2);
        assert_eq!(scheduler.usage().generated_nodes, 0);
        assert_eq!(scheduler.problems().len(), 2);
        assert_eq!(
            scheduler.problems()[0].reason,
            RecoveryReason::ResourceLimit(ResourceLimitKind::InterpreterSteps)
        );
        assert_eq!(
            scheduler.problems()[1].reason,
            RecoveryReason::ResourceLimit(ResourceLimitKind::LiveValueBytes)
        );
        assert_eq!(
            scheduler.work()[1]
                .resources
                .expect("execution usage is retained")
                .peak_live_value_bytes,
            4
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn arbitrary_generated_chains_stop_at_the_configured_depth(
            requested_children in 0u32..32,
            depth_limit in 1u32..16,
        ) {
            let fixture = Fixture::new();
            let mut scheduler = ExpansionScheduler::with_limits(ExpansionLimits {
                active_depth: depth_limit,
                ..ExpansionLimits::default()
            });
            scheduler.enqueue(fixture.request(
                0,
                ExpansionRole::Expression,
                "root",
                &[],
                &[0],
            ));
            let mut visited = 0u32;
            scheduler.run(|work, _| {
                visited += 1;
                if work.depth <= requested_children {
                    Ok(ScheduledOutput {
                        generated_nodes: 1,
                        invocations: vec![fixture.request(
                            work.depth,
                            ExpansionRole::Expression,
                            "changing-declaration",
                            &[],
                            &work.request.location.generated(0).provenance_order,
                        )],
                        definitions: Vec::new(),
                    })
                } else {
                    Ok(ScheduledOutput::default())
                }
            });

            let expected_executions = (requested_children + 1).min(depth_limit);
            prop_assert_eq!(visited, expected_executions);
            prop_assert_eq!(scheduler.usage().executions, u64::from(expected_executions));
            if requested_children + 1 > depth_limit {
                prop_assert_eq!(scheduler.work().len(), depth_limit as usize + 1);
                prop_assert_eq!(
                    scheduler.problems().last().map(|problem| problem.reason),
                    Some(RecoveryReason::ResourceLimit(ResourceLimitKind::ActiveDepth)),
                );
            } else {
                prop_assert_eq!(scheduler.work().len(), requested_children as usize + 1);
                prop_assert!(scheduler.problems().is_empty());
            }
        }
    }
}
