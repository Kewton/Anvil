//! Issue #1003: `ScaffoldProfile` registry — a deterministic, table-driven SSOT
//! for the minimal scaffolds materialized per task runtime/use (coding / docs /
//! data / research).
//!
//! Motivation (v0.6.5): Node CSV tasks generated a test file but no
//! `package.json` / test script and fell into `missing_verification`; Rust tasks
//! drifted on manifest / crate / bin / test bindings. That is a *scaffold*
//! problem, not a repair problem — the controller should fix the shape with a
//! deterministic profile rather than letting the LLM free-generate it.
//!
//! Design:
//!
//! * The registry owns one [`ScaffoldProfile`] per [`ScaffoldProfileId`]. Each
//!   profile declares its [`RuntimeKind`], the telemetry `label / event /
//!   scaffold_kind`, the required artifact roles, the verification
//!   [`CommandTemplate`], and the [`BindingCheck`]s its materialized output must
//!   satisfy.
//! * Coding profiles (Rust CLI/lib, Node CLI/lib) **delegate** materialization to
//!   the existing skeleton generators in [`super::scaffold_pipeline`] so the byte
//!   output stays identical to the pre-registry path and the dynamic entrypoint
//!   keeps working. Non-coding profiles (Python, FastAPI, docs, data, research)
//!   materialize from small `&'static` [`FileTemplate`] tables owned here.
//! * `plan_project_skeleton_with_obligations` (in `scaffold_pipeline`) routes the
//!   empty-workspace coding skeletons through [`ScaffoldProfile::for_runtime_shape`]
//!   and asserts [`ScaffoldProfile::verify_bindings`] on the materialized files,
//!   so the binding contract is a production invariant.
//!
//! `pub(super)` limited / no facade re-export (DR3-001). No provider abstraction.

use std::path::{Path, PathBuf};

use crate::session::store::ScaffoldArtifactRole;

use super::scaffold_pipeline::{
    ProjectRuntime, ProjectShape, node_skeleton_files, rust_cli_skeleton_files,
    rust_library_skeleton_files, scaffold_role_for_path,
};

/// Coarse runtime/use bucket a profile belongs to. Spans coding runtimes and the
/// non-coding uses (docs / data / research) so every scaffold lives in one
/// registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeKind {
    Python,
    Node,
    Rust,
    Docs,
    Data,
    Research,
}

/// Stable identifier for each registered profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScaffoldProfileId {
    PythonCliPytest,
    NodeCliNodeTest,
    NodeLibNodeTest,
    RustCliCargo,
    RustLibCargo,
    FastApiPytest,
    DocsMarkdownCheck,
    CsvTransformCheck,
    ResearchNotesCheck,
}

impl ScaffoldProfileId {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::PythonCliPytest => "python_cli_pytest",
            Self::NodeCliNodeTest => "node_cli_node_test",
            Self::NodeLibNodeTest => "node_lib_node_test",
            Self::RustCliCargo => "rust_cli_cargo",
            Self::RustLibCargo => "rust_lib_cargo",
            Self::FastApiPytest => "fastapi_pytest",
            Self::DocsMarkdownCheck => "docs_markdown_check",
            Self::CsvTransformCheck => "csv_transform_check",
            Self::ResearchNotesCheck => "research_notes_check",
        }
    }
}

/// A single deterministic file the profile materializes. `role` mirrors the
/// session-layer [`ScaffoldArtifactRole`] so the produced files stay consistent
/// with the existing scaffold artifact snapshot. The declared `role` is checked
/// against the path classification at materialize time (see [`template_files`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct FileTemplate {
    pub(super) path: &'static str,
    pub(super) role: ScaffoldArtifactRole,
    pub(super) content: &'static str,
}

/// The verification command a profile expects to be runnable once materialized.
#[derive(Debug, Clone, Copy)]
pub(super) struct CommandTemplate {
    pub(super) program: &'static str,
    pub(super) args: &'static [&'static str],
}

impl CommandTemplate {
    pub(super) fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.to_string()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// Declarative invariant a profile's materialized output must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingCheck {
    /// The profile declares a non-empty verification command.
    VerificationCommandDeclared,
    /// Every role in `required_roles` is present among the materialized files.
    RequiredRolesPresent,
    /// If a test-role file is materialized, a `package.json` declaring a `test`
    /// script must be materialized too (fixes the v0.6.5 Node CSV hole).
    NodeManifestDeclaresTestScript,
    /// `Cargo.toml` must declare a `path` that resolves to a materialized file
    /// (the bin/lib entrypoint binding).
    CargoManifestBindsEntrypoint,
}

/// One registry entry. `pub(super)` fields so `scaffold_pipeline` can read the
/// telemetry metadata when building a `ScaffoldPlan`.
pub(super) struct ScaffoldProfile {
    pub(super) id: ScaffoldProfileId,
    pub(super) runtime: RuntimeKind,
    pub(super) label: &'static str,
    pub(super) event: &'static str,
    pub(super) scaffold_kind: &'static str,
    pub(super) required_roles: &'static [ScaffoldArtifactRole],
    pub(super) command: CommandTemplate,
    pub(super) binding_checks: &'static [BindingCheck],
}

impl ScaffoldProfile {
    pub(super) fn as_str(&self) -> &'static str {
        self.id.as_str()
    }

    /// Default entrypoint used when the request carries no explicit obligation
    /// path. Only meaningful for the delegated coding profiles; template profiles
    /// ignore the entrypoint and return `""`.
    pub(super) fn default_entrypoint(&self) -> &'static str {
        match self.id {
            ScaffoldProfileId::RustCliCargo => "src/main.rs",
            ScaffoldProfileId::RustLibCargo => "src/lib.rs",
            ScaffoldProfileId::NodeCliNodeTest | ScaffoldProfileId::NodeLibNodeTest => {
                "src/index.js"
            }
            _ => "",
        }
    }

    /// Materialize the profile into `(relative_path, content)` pairs. Coding
    /// profiles delegate to the shared skeleton generators (honoring the dynamic
    /// `entrypoint`); the rest expand their `&'static` template tables.
    pub(super) fn materialize(&self, entrypoint: Option<&Path>) -> Vec<(PathBuf, String)> {
        match self.id {
            ScaffoldProfileId::RustCliCargo => {
                rust_cli_skeleton_files(resolve_entrypoint(entrypoint, "src/main.rs"))
            }
            ScaffoldProfileId::RustLibCargo => {
                rust_library_skeleton_files(resolve_entrypoint(entrypoint, "src/lib.rs"))
            }
            ScaffoldProfileId::NodeCliNodeTest => node_skeleton_files(
                ProjectShape::Cli,
                resolve_entrypoint(entrypoint, "src/index.js"),
            ),
            ScaffoldProfileId::NodeLibNodeTest => node_skeleton_files(
                ProjectShape::Library,
                resolve_entrypoint(entrypoint, "src/index.js"),
            ),
            ScaffoldProfileId::PythonCliPytest => template_files(PYTHON_CLI_PYTEST_TEMPLATES),
            ScaffoldProfileId::FastApiPytest => template_files(FASTAPI_PYTEST_TEMPLATES),
            ScaffoldProfileId::DocsMarkdownCheck => template_files(DOCS_MARKDOWN_TEMPLATES),
            ScaffoldProfileId::CsvTransformCheck => template_files(CSV_TRANSFORM_TEMPLATES),
            ScaffoldProfileId::ResearchNotesCheck => template_files(RESEARCH_NOTES_TEMPLATES),
        }
    }

    /// Verify the materialized `files` satisfy every declared [`BindingCheck`].
    /// Returns the first violation's description so callers can surface it.
    pub(super) fn verify_bindings(&self, files: &[(PathBuf, String)]) -> Result<(), String> {
        for check in self.binding_checks {
            match check {
                BindingCheck::VerificationCommandDeclared => {
                    if self.command.program.is_empty() {
                        return Err(format!(
                            "{}: no verification command declared",
                            self.as_str()
                        ));
                    }
                }
                BindingCheck::RequiredRolesPresent => {
                    for role in self.required_roles {
                        let present = files
                            .iter()
                            .any(|(path, _)| scaffold_role_for_path(path) == *role);
                        if !present {
                            return Err(format!(
                                "{}: required role {:?} not materialized",
                                self.as_str(),
                                role
                            ));
                        }
                    }
                }
                BindingCheck::NodeManifestDeclaresTestScript => {
                    let has_test_file = files.iter().any(|(path, _)| {
                        scaffold_role_for_path(path) == ScaffoldArtifactRole::Test
                    });
                    if has_test_file && !manifest_declares_test_script(files) {
                        return Err(format!(
                            "{}: test file present but package.json declares no test script",
                            self.as_str()
                        ));
                    }
                }
                BindingCheck::CargoManifestBindsEntrypoint => {
                    let Some((_, manifest)) = files
                        .iter()
                        .find(|(path, _)| file_name(path) == Some("Cargo.toml"))
                    else {
                        return Err(format!("{}: Cargo.toml not materialized", self.as_str()));
                    };
                    let bound = declared_cargo_path(manifest)
                        .map(|rel| files.iter().any(|(path, _)| path == &PathBuf::from(&rel)))
                        .unwrap_or(false);
                    if !bound {
                        return Err(format!(
                            "{}: Cargo.toml entrypoint path is not bound to a materialized file",
                            self.as_str()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Registry lookup for a coding `(runtime, shape)` pair.
    pub(super) fn for_runtime_shape(
        runtime: ProjectRuntime,
        shape: ProjectShape,
    ) -> &'static ScaffoldProfile {
        let id = match (runtime, shape) {
            (ProjectRuntime::Rust, ProjectShape::Cli) => ScaffoldProfileId::RustCliCargo,
            (ProjectRuntime::Rust, ProjectShape::Library) => ScaffoldProfileId::RustLibCargo,
            (ProjectRuntime::Node, ProjectShape::Cli) => ScaffoldProfileId::NodeCliNodeTest,
            (ProjectRuntime::Node, ProjectShape::Library) => ScaffoldProfileId::NodeLibNodeTest,
        };
        profile(id)
    }
}

/// Resolve `id` to its registry entry. Total over [`ScaffoldProfileId`] by
/// construction (the table-driven test pins totality).
pub(super) fn profile(id: ScaffoldProfileId) -> &'static ScaffoldProfile {
    PROFILES
        .iter()
        .find(|profile| profile.id == id)
        .expect("every ScaffoldProfileId has a registry entry")
}

fn resolve_entrypoint(entrypoint: Option<&Path>, default: &str) -> PathBuf {
    entrypoint
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn template_files(templates: &'static [FileTemplate]) -> Vec<(PathBuf, String)> {
    templates
        .iter()
        .map(|tpl| {
            let path = PathBuf::from(tpl.path);
            debug_assert_eq!(
                scaffold_role_for_path(&path),
                tpl.role,
                "scaffold template {} declares role {:?} but path classifies differently",
                tpl.path,
                tpl.role
            );
            (path, tpl.content.to_string())
        })
        .collect()
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn manifest_declares_test_script(files: &[(PathBuf, String)]) -> bool {
    files.iter().any(|(path, content)| {
        file_name(path) == Some("package.json") && content.contains("\"test\"")
    })
}

/// Extract the first `path = "<x>"` value from a Cargo manifest body.
fn declared_cargo_path(manifest: &str) -> Option<String> {
    for line in manifest.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("path") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        let Some(inner) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = inner.find('"') {
            return Some(inner[..end].to_string());
        }
    }
    None
}

const PYTHON_CLI_PYTEST_TEMPLATES: &[FileTemplate] = &[
    FileTemplate {
        path: "main.py",
        role: ScaffoldArtifactRole::Implementation,
        content: r#""""Minimal CLI entrypoint."""
import sys


def transform(value: str) -> str:
    """Return the input unchanged. Replace with the requested behavior."""
    return value


def main() -> None:
    sys.stdout.write(transform(sys.stdin.read()))


if __name__ == "__main__":
    main()
"#,
    },
    FileTemplate {
        path: "tests/test_main.py",
        role: ScaffoldArtifactRole::Test,
        content: r#"from main import transform


def test_transform_passes_input_through():
    assert transform("sample input") == "sample input"
"#,
    },
    FileTemplate {
        path: "README.md",
        role: ScaffoldArtifactRole::UsageDocs,
        content: r#"# Python CLI Skeleton

## Run

```bash
echo "sample input" | python3 main.py
```

## Test

```bash
pytest
```

Replace the neutral `transform` implementation with the requested behavior.
"#,
    },
];

const FASTAPI_PYTEST_TEMPLATES: &[FileTemplate] = &[
    FileTemplate {
        path: "pyproject.toml",
        role: ScaffoldArtifactRole::Setup,
        content: r#"[project]
name = "fastapi-app"
version = "0.1.0"
description = "FastAPI application scaffold"
requires-python = ">=3.9"
dependencies = ["fastapi", "httpx", "pytest"]
"#,
    },
    FileTemplate {
        path: "app/main.py",
        role: ScaffoldArtifactRole::Implementation,
        content: r#"from fastapi import FastAPI

app = FastAPI()


@app.get("/health")
def health() -> dict:
    return {"status": "ok"}
"#,
    },
    FileTemplate {
        path: "tests/test_app.py",
        role: ScaffoldArtifactRole::Test,
        content: r#"from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)


def test_health_returns_ok():
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}
"#,
    },
    FileTemplate {
        path: "README.md",
        role: ScaffoldArtifactRole::UsageDocs,
        content: r#"# FastAPI Skeleton

## Run

```bash
uvicorn app.main:app --reload
```

## Test

```bash
pytest
```

Replace the `/health` route with the requested API behavior.
"#,
    },
];

const DOCS_MARKDOWN_TEMPLATES: &[FileTemplate] = &[
    FileTemplate {
        path: "README.md",
        role: ScaffoldArtifactRole::UsageDocs,
        content: r#"# Project Documentation

## Overview

Describe the project here.

## Usage

Document how to use the project.
"#,
    },
    FileTemplate {
        path: "docs/overview.md",
        role: ScaffoldArtifactRole::UsageDocs,
        content: r#"# Overview

Expand the project documentation in this file.
"#,
    },
];

const CSV_TRANSFORM_TEMPLATES: &[FileTemplate] = &[
    FileTemplate {
        path: "transform.py",
        role: ScaffoldArtifactRole::Implementation,
        content: r#"#!/usr/bin/env python3
"""Summarize Amount totals grouped by Category from a CSV file."""
import csv
import sys
from collections import defaultdict


def summarize(path: str) -> dict:
    totals: dict = defaultdict(float)
    with open(path, newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            category = (row.get("Category") or "Uncategorized").strip() or "Uncategorized"
            amount = (row.get("Amount") or "0").replace(",", "").strip()
            totals[category] += float(amount)
    return dict(totals)


def main() -> None:
    for category, total in sorted(summarize(sys.argv[1]).items()):
        print(f"{category},{total:.2f}")


if __name__ == "__main__":
    main()
"#,
    },
    FileTemplate {
        path: "sample.csv",
        role: ScaffoldArtifactRole::Other,
        content: "Category,Amount\nFood,1200\nTransport,450\nFood,800\nBooks,2500\n",
    },
    FileTemplate {
        path: "README.md",
        role: ScaffoldArtifactRole::UsageDocs,
        content: r#"# CSV Transform

## Run

```bash
python3 transform.py sample.csv
```

The input CSV must include `Category` and `Amount` columns.
"#,
    },
];

const RESEARCH_NOTES_TEMPLATES: &[FileTemplate] = &[FileTemplate {
    path: "research-notes.md",
    role: ScaffoldArtifactRole::UsageDocs,
    content: r#"# Research Notes

## Question

State the research question.

## Findings

Summarize findings with citations.

## Uncertainty

Note open questions and remaining confidence.
"#,
}];

static PROFILES: &[ScaffoldProfile] = &[
    ScaffoldProfile {
        id: ScaffoldProfileId::PythonCliPytest,
        runtime: RuntimeKind::Python,
        label: "Python CLI scaffold",
        event: "agent.empty_workspace.deterministic_python_cli_pytest",
        scaffold_kind: "Python CLI",
        required_roles: &[
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "pytest",
            args: &[],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::NodeCliNodeTest,
        runtime: RuntimeKind::Node,
        label: "Node CLI scaffold",
        event: "agent.empty_workspace.deterministic_node_cli",
        scaffold_kind: "Node CLI",
        required_roles: &[
            ScaffoldArtifactRole::Setup,
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "node",
            args: &["--test"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
            BindingCheck::NodeManifestDeclaresTestScript,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::NodeLibNodeTest,
        runtime: RuntimeKind::Node,
        label: "Node library scaffold",
        event: "agent.empty_workspace.deterministic_node_library",
        scaffold_kind: "Node library",
        required_roles: &[
            ScaffoldArtifactRole::Setup,
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "node",
            args: &["--test"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
            BindingCheck::NodeManifestDeclaresTestScript,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::RustCliCargo,
        runtime: RuntimeKind::Rust,
        label: "Rust CLI scaffold",
        event: "agent.empty_workspace.deterministic_rust_cli",
        scaffold_kind: "Rust CLI",
        required_roles: &[
            ScaffoldArtifactRole::Setup,
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "cargo",
            args: &["test"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
            BindingCheck::CargoManifestBindsEntrypoint,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::RustLibCargo,
        runtime: RuntimeKind::Rust,
        label: "Rust library scaffold",
        event: "agent.empty_workspace.deterministic_rust_library",
        scaffold_kind: "Rust library",
        required_roles: &[
            ScaffoldArtifactRole::Setup,
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "cargo",
            args: &["test"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
            BindingCheck::CargoManifestBindsEntrypoint,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::FastApiPytest,
        runtime: RuntimeKind::Python,
        label: "FastAPI scaffold",
        event: "agent.empty_workspace.deterministic_fastapi_scaffold",
        scaffold_kind: "FastAPI",
        required_roles: &[
            ScaffoldArtifactRole::Setup,
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::Test,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "pytest",
            args: &[],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::DocsMarkdownCheck,
        runtime: RuntimeKind::Docs,
        label: "Docs scaffold",
        event: "agent.empty_workspace.deterministic_docs",
        scaffold_kind: "Docs",
        required_roles: &[ScaffoldArtifactRole::UsageDocs],
        command: CommandTemplate {
            program: "test",
            args: &["-s", "README.md"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::CsvTransformCheck,
        runtime: RuntimeKind::Data,
        label: "CSV transform scaffold",
        event: "agent.empty_workspace.deterministic_csv_transform",
        scaffold_kind: "CSV transform",
        required_roles: &[
            ScaffoldArtifactRole::Implementation,
            ScaffoldArtifactRole::UsageDocs,
        ],
        command: CommandTemplate {
            program: "python3",
            args: &["transform.py", "sample.csv"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
        ],
    },
    ScaffoldProfile {
        id: ScaffoldProfileId::ResearchNotesCheck,
        runtime: RuntimeKind::Research,
        label: "Research notes scaffold",
        event: "agent.empty_workspace.deterministic_research_notes",
        scaffold_kind: "Research notes",
        required_roles: &[ScaffoldArtifactRole::UsageDocs],
        command: CommandTemplate {
            program: "test",
            args: &["-s", "research-notes.md"],
        },
        binding_checks: &[
            BindingCheck::VerificationCommandDeclared,
            BindingCheck::RequiredRolesPresent,
        ],
    },
];

#[cfg(test)]
pub(super) const ALL_PROFILE_IDS: &[ScaffoldProfileId] = &[
    ScaffoldProfileId::PythonCliPytest,
    ScaffoldProfileId::NodeCliNodeTest,
    ScaffoldProfileId::NodeLibNodeTest,
    ScaffoldProfileId::RustCliCargo,
    ScaffoldProfileId::RustLibCargo,
    ScaffoldProfileId::FastApiPytest,
    ScaffoldProfileId::DocsMarkdownCheck,
    ScaffoldProfileId::CsvTransformCheck,
    ScaffoldProfileId::ResearchNotesCheck,
];
