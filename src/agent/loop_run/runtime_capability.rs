//! Runtime capability packets for contract-bound generation.
//!
//! The packet is an observed controller-side fact about the local runner. It is
//! intentionally stack-level rather than benchmark-level: generation can avoid
//! impossible dependencies without adding per-task repair branches.

use std::process::Command;

use super::worker_contract::{RuntimeProfile, TaskExecutionContract};
use crate::session::store::ConversationMessage;

const MAX_VERSION_BYTES: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeCapabilityPacket {
    profile: RuntimeProfile,
    runner: &'static str,
    available: bool,
    observed_version: Option<String>,
    unavailable_capabilities: Vec<&'static str>,
}

impl RuntimeCapabilityPacket {
    fn unavailable_labels(&self) -> Vec<&'static str> {
        self.unavailable_capabilities.clone()
    }

    fn policy_message(&self) -> String {
        let unavailable = self.unavailable_labels();
        let unavailable = if unavailable.is_empty() {
            "none".to_string()
        } else {
            unavailable.join(",")
        };
        let version = self.observed_version.as_deref().unwrap_or("unknown");
        format!(
            "[Runtime Capability Packet] runtime={}; runner={}; runner_available={}; observed_version={}; unavailable_capabilities={}. Treat this as controller-observed fact. Do not import, install, or test against unavailable runtime capabilities unless the ObjectiveContract explicitly requires setup for them; choose an implementation compatible with the observed runner.",
            self.profile.label(),
            self.runner,
            self.available,
            version,
            unavailable
        )
    }
}

pub(super) fn runtime_capability_message_for_execution(
    execution: &TaskExecutionContract,
) -> Option<ConversationMessage> {
    RuntimeCapabilityPacket::probe(execution.runtime_profile)
        .map(|packet| ConversationMessage::system(packet.policy_message()))
}

impl RuntimeCapabilityPacket {
    fn probe(profile: RuntimeProfile) -> Option<Self> {
        match profile {
            RuntimeProfile::Python => Some(probe_python3()),
            RuntimeProfile::Rust => Some(probe_version_command(profile, "cargo", &["--version"])),
            RuntimeProfile::Node | RuntimeProfile::TypeScript => {
                Some(probe_version_command(profile, "node", &["--version"]))
            }
            RuntimeProfile::Unspecified => None,
        }
    }
}

fn probe_python3() -> RuntimeCapabilityPacket {
    let mut packet = probe_version_command(RuntimeProfile::Python, "python3", &["--version"]);
    if !packet.available {
        return packet;
    }
    let version = packet
        .observed_version
        .as_deref()
        .and_then(parse_python_version);
    packet.unavailable_capabilities = python_unavailable_capabilities(version);
    packet
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityRequirement {
    label: &'static str,
    minimum_version: RuntimeVersion,
}

const PYTHON_CAPABILITY_REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    label: "python_stdlib.tomllib(min_python=3.11)",
    minimum_version: RuntimeVersion {
        major: 3,
        minor: 11,
    },
}];

fn python_unavailable_capabilities(version: Option<RuntimeVersion>) -> Vec<&'static str> {
    PYTHON_CAPABILITY_REQUIREMENTS
        .iter()
        .filter(|requirement| {
            !version.is_some_and(|version| version.at_least_requirement(requirement))
        })
        .map(|requirement| requirement.label)
        .collect()
}

fn probe_version_command(
    profile: RuntimeProfile,
    runner: &'static str,
    args: &[&str],
) -> RuntimeCapabilityPacket {
    match Command::new(runner).args(args).output() {
        Ok(output) => RuntimeCapabilityPacket {
            profile,
            runner,
            available: output.status.success(),
            observed_version: first_non_empty_version_line(&output.stdout, &output.stderr),
            unavailable_capabilities: Vec::new(),
        },
        Err(_) => RuntimeCapabilityPacket {
            profile,
            runner,
            available: false,
            observed_version: None,
            unavailable_capabilities: Vec::new(),
        },
    }
}

fn first_non_empty_version_line(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let combined = [stdout, stderr].concat();
    String::from_utf8_lossy(&combined)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(cap_version_line)
}

fn cap_version_line(line: &str) -> String {
    let mut capped = line.chars().take(MAX_VERSION_BYTES).collect::<String>();
    if capped.len() < line.len() {
        capped.push_str("...");
    }
    capped
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeVersion {
    major: u32,
    minor: u32,
}

impl RuntimeVersion {
    fn at_least(self, major: u32, minor: u32) -> bool {
        self.major > major || (self.major == major && self.minor >= minor)
    }

    fn at_least_requirement(self, requirement: &CapabilityRequirement) -> bool {
        self.at_least(
            requirement.minimum_version.major,
            requirement.minimum_version.minor,
        )
    }
}

fn parse_python_version(raw: &str) -> Option<RuntimeVersion> {
    let version = raw
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()))?;
    let mut parts = version.split('.');
    Some(RuntimeVersion {
        major: parts.next()?.parse().ok()?,
        minor: parts.next()?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_version_parser_accepts_stdout_shape() {
        assert_eq!(
            parse_python_version("Python 3.9.6"),
            Some(RuntimeVersion { major: 3, minor: 9 })
        );
        assert_eq!(
            parse_python_version("Python 3.11.1"),
            Some(RuntimeVersion {
                major: 3,
                minor: 11
            })
        );
    }

    #[test]
    fn python_packet_marks_tomllib_unavailable_before_3_11() {
        let packet = RuntimeCapabilityPacket {
            profile: RuntimeProfile::Python,
            runner: "python3",
            available: true,
            observed_version: Some("Python 3.9.6".to_string()),
            unavailable_capabilities: vec!["python_stdlib.tomllib(min_python=3.11)"],
        };

        let message = packet.policy_message();
        assert!(message.contains("runtime=python"));
        assert!(message.contains("python_stdlib.tomllib(min_python=3.11)"));
        assert!(message.contains("controller-observed fact"));
    }

    #[test]
    fn python_capability_table_marks_requirements_by_version() {
        assert_eq!(
            python_unavailable_capabilities(Some(RuntimeVersion { major: 3, minor: 9 })),
            vec!["python_stdlib.tomllib(min_python=3.11)"]
        );
        assert!(
            python_unavailable_capabilities(Some(RuntimeVersion {
                major: 3,
                minor: 11
            }))
            .is_empty()
        );
    }

    #[test]
    fn unspecified_runtime_does_not_emit_capability_packet() {
        let contract =
            super::super::task_contract::TaskContract::from_request("Write README.md only.");
        let execution = TaskExecutionContract::from_task_contract(&contract);

        assert!(runtime_capability_message_for_execution(&execution).is_none());
    }
}
