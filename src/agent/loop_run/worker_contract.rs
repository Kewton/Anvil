//! Issue #961: task-specialized worker contracts and bounded context packs.
//!
//! This is an additive foundation. It deliberately does not dispatch workers
//! yet; later MissingEvidence / DiagnosticRepair / non-coding capability
//! tracks can consume the same contract shape without adding provider
//! abstraction or broad prompt/context formats.

#![allow(dead_code)] // Foundation seam; focused tests pin the shape before broad callers are wired.

use super::task_contract::{
    ObjectiveDeliverableKind, ObjectiveEvidenceKind, TaskContract, TaskKind,
};

pub(super) const MAX_CONTEXT_PACK_ENTRIES: usize = 8;
pub(super) const MAX_CONTEXT_PACK_ENTRY_BYTES: usize = 2048;
const MAX_CONTEXT_PACK_LABEL_BYTES: usize = 128;
const TRUNCATION_MARKER: &str = "...";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerKind {
    Implement,
    TestAuthor,
    Evidence,
    DiagnosticRepair,
    Docs,
    Data,
    Research,
    Ops,
    Authoring,
}

impl WorkerKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            WorkerKind::Implement => "implement",
            WorkerKind::TestAuthor => "test_author",
            WorkerKind::Evidence => "evidence",
            WorkerKind::DiagnosticRepair => "diagnostic_repair",
            WorkerKind::Docs => "docs",
            WorkerKind::Data => "data",
            WorkerKind::Research => "research",
            WorkerKind::Ops => "ops",
            WorkerKind::Authoring => "authoring",
        }
    }

    pub(super) fn primary_for_task_kind(task_kind: TaskKind) -> Self {
        match task_kind {
            TaskKind::Coding => WorkerKind::Implement,
            TaskKind::Docs => WorkerKind::Docs,
            TaskKind::Data => WorkerKind::Data,
            TaskKind::Research => WorkerKind::Research,
            TaskKind::Ops => WorkerKind::Ops,
            TaskKind::Authoring => WorkerKind::Authoring,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContextPackKind {
    Contract,
    Target,
    Evidence,
    Diagnostic,
    Repair,
}

impl ContextPackKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            ContextPackKind::Contract => "contract",
            ContextPackKind::Target => "target",
            ContextPackKind::Evidence => "evidence",
            ContextPackKind::Diagnostic => "diagnostic",
            ContextPackKind::Repair => "repair",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContextPackEntry {
    pub(super) kind: ContextPackKind,
    label: String,
    content: String,
    truncated: bool,
}

impl ContextPackEntry {
    pub(super) fn new(
        kind: ContextPackKind,
        label: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let (label, _) = truncate_utf8(label.into(), MAX_CONTEXT_PACK_LABEL_BYTES);
        let (content, truncated) = truncate_utf8(content.into(), MAX_CONTEXT_PACK_ENTRY_BYTES);
        Self {
            kind,
            label,
            content,
            truncated,
        }
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn content(&self) -> &str {
        &self.content
    }

    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ContextPack {
    entries: Vec<ContextPackEntry>,
}

impl ContextPack {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn from_entries(entries: impl IntoIterator<Item = ContextPackEntry>) -> Self {
        let mut pack = Self::new();
        for entry in entries {
            pack.push(entry);
        }
        pack
    }

    pub(super) fn push(&mut self, entry: ContextPackEntry) {
        if self.entries.len() < MAX_CONTEXT_PACK_ENTRIES {
            self.entries.push(entry);
        }
    }

    pub(super) fn entries(&self) -> &[ContextPackEntry] {
        &self.entries
    }

    pub(super) fn entries_for_kind(&self, kind: ContextPackKind) -> Vec<&ContextPackEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .collect()
    }

    pub(super) fn approximate_token_count(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| {
                approximate_token_count(entry.label()) + approximate_token_count(entry.content())
            })
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkerContract {
    pub(super) worker_kind: WorkerKind,
    pub(super) task_kind: TaskKind,
    pub(super) deliverable_kind: ObjectiveDeliverableKind,
    pub(super) evidence_kind: ObjectiveEvidenceKind,
    pub(super) context_pack: ContextPack,
}

impl WorkerContract {
    pub(super) fn from_task_contract(contract: &TaskContract, worker_kind: WorkerKind) -> Self {
        let objective = contract.objective_contract();
        let context_pack = ContextPack::from_entries([
            ContextPackEntry::new(
                ContextPackKind::Contract,
                "task_kind",
                objective.task_kind.as_str(),
            ),
            ContextPackEntry::new(
                ContextPackKind::Contract,
                "deliverable_kind",
                objective.deliverable_kind.label(),
            ),
            ContextPackEntry::new(
                ContextPackKind::Evidence,
                "evidence_kind",
                objective.evidence_kind.label(),
            ),
        ]);
        Self {
            worker_kind,
            task_kind: objective.task_kind,
            deliverable_kind: objective.deliverable_kind,
            evidence_kind: objective.evidence_kind,
            context_pack,
        }
    }

    pub(super) fn primary_for_task_contract(contract: &TaskContract) -> Self {
        let objective = contract.objective_contract();
        Self::from_task_contract(
            contract,
            WorkerKind::primary_for_task_kind(objective.task_kind),
        )
    }

    pub(super) fn with_context_pack(mut self, context_pack: ContextPack) -> Self {
        self.context_pack = context_pack;
        self
    }
}

fn truncate_utf8(value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = value[..end].to_string();
    truncated.push_str(TRUNCATION_MARKER);
    (truncated, true)
}

fn approximate_token_count(text: &str) -> usize {
    text.split_whitespace().count().max(text.len().div_ceil(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_kind_labels_cover_current_worker_intents() {
        let labels = [
            WorkerKind::Implement.label(),
            WorkerKind::TestAuthor.label(),
            WorkerKind::Evidence.label(),
            WorkerKind::DiagnosticRepair.label(),
            WorkerKind::Docs.label(),
            WorkerKind::Data.label(),
            WorkerKind::Research.label(),
            WorkerKind::Ops.label(),
            WorkerKind::Authoring.label(),
        ];
        assert_eq!(
            labels,
            [
                "implement",
                "test_author",
                "evidence",
                "diagnostic_repair",
                "docs",
                "data",
                "research",
                "ops",
                "authoring"
            ]
        );
    }

    #[test]
    fn worker_contract_carries_objective_vocabulary_for_coding_test_author() {
        let contract = TaskContract::from_request(
            "Create a Rust CLI word counter with Cargo tests and usage docs.",
        );
        let worker = WorkerContract::from_task_contract(&contract, WorkerKind::TestAuthor);

        assert_eq!(worker.worker_kind, WorkerKind::TestAuthor);
        assert_eq!(worker.task_kind, TaskKind::Coding);
        assert_eq!(
            worker.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        );
        assert_eq!(worker.evidence_kind, ObjectiveEvidenceKind::TestRun);
        assert_eq!(
            worker
                .context_pack
                .entries_for_kind(ContextPackKind::Contract)
                .len(),
            2
        );
        assert_eq!(
            worker
                .context_pack
                .entries_for_kind(ContextPackKind::Evidence)
                .len(),
            1
        );
    }

    #[test]
    fn primary_worker_contract_covers_non_coding_task_kinds() {
        let cases = [
            (
                "Write README.md with prerequisites, rollback, validation, and incident sections.",
                WorkerKind::Docs,
                ObjectiveDeliverableKind::DocumentSections,
                ObjectiveEvidenceKind::ContentCheck,
            ),
            (
                "Transform orders.csv into cleaned output.csv with id,total columns.",
                WorkerKind::Data,
                ObjectiveDeliverableKind::OutputFile,
                ObjectiveEvidenceKind::SchemaCheck,
            ),
            (
                "Research local LLM repair loops and draft a report with sources.",
                WorkerKind::Research,
                ObjectiveDeliverableKind::ResearchNotes,
                ObjectiveEvidenceKind::SourceFetchEvidence,
            ),
        ];

        for (request, expected_worker, expected_deliverable, expected_evidence) in cases {
            let contract = TaskContract::from_request(request);
            let worker = WorkerContract::primary_for_task_contract(&contract);
            assert_eq!(worker.worker_kind, expected_worker, "{request}");
            assert_eq!(worker.deliverable_kind, expected_deliverable, "{request}");
            assert_eq!(worker.evidence_kind, expected_evidence, "{request}");
        }
    }

    #[test]
    fn context_pack_bounds_entries_and_filters_by_kind() {
        let long = "x".repeat(MAX_CONTEXT_PACK_ENTRY_BYTES + 32);
        let entries = (0..MAX_CONTEXT_PACK_ENTRIES + 3).map(|idx| {
            let kind = if idx % 2 == 0 {
                ContextPackKind::Target
            } else {
                ContextPackKind::Repair
            };
            ContextPackEntry::new(kind, format!("entry-{idx}"), long.clone())
        });
        let pack = ContextPack::from_entries(entries);

        assert_eq!(pack.entries().len(), MAX_CONTEXT_PACK_ENTRIES);
        assert!(pack.entries()[0].truncated());
        assert!(
            pack.entries()[0].content().len()
                <= MAX_CONTEXT_PACK_ENTRY_BYTES + TRUNCATION_MARKER.len()
        );
        assert_eq!(
            pack.entries_for_kind(ContextPackKind::Target).len(),
            MAX_CONTEXT_PACK_ENTRIES / 2
        );
        assert!(pack.approximate_token_count() > 0);
    }
}
