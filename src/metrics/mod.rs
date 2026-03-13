use std::fmt::{Display, Formatter};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkTarget {
    Anvil,
    VibeLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonAxis {
    FirstUseExperience,
    IterationSpeed,
    StabilityAndRecovery,
    LongSessionUsability,
    UxClarity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementKind {
    DurationMs,
    Count,
    Score5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferredDirection {
    LowerIsBetter,
    HigherIsBetter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementSource {
    Measured,
    OperationalScore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioDefinition {
    pub id: &'static str,
    pub axis: ComparisonAxis,
    pub title: &'static str,
    pub metric_name: &'static str,
    pub measurement: MeasurementKind,
    pub preferred_direction: PreferredDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasurementRecord {
    pub target: BenchmarkTarget,
    pub scenario_id: String,
    pub value: u32,
    pub source: MeasurementSource,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonOutcome {
    pub scenario_id: String,
    pub axis: ComparisonAxis,
    pub winner: Option<BenchmarkTarget>,
    pub anvil_value: Option<u32>,
    pub vibe_local_value: Option<u32>,
}

#[derive(Debug, Default)]
pub struct MetricsRegistry {
    scenarios: Vec<ScenarioDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBenchmark<'a> {
    program: &'a str,
    args: Vec<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBenchmarkResult {
    pub runs_ms: Vec<u32>,
    pub average_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBenchmarkError {
    message: String,
}

impl Display for CommandBenchmarkError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CommandBenchmarkError {}

impl<'a> CommandBenchmark<'a> {
    pub fn new(program: &'a str, args: &[&'a str]) -> Self {
        Self {
            program,
            args: args.to_vec(),
        }
    }

    pub fn run(&self, runs: usize) -> Result<CommandBenchmarkResult, CommandBenchmarkError> {
        let mut results = Vec::new();

        for _ in 0..runs {
            let started = Instant::now();
            let status = Command::new(self.program)
                .args(&self.args)
                .status()
                .map_err(|err| CommandBenchmarkError {
                    message: format!("failed to run benchmark command: {err}"),
                })?;
            if !status.success() {
                return Err(CommandBenchmarkError {
                    message: format!("benchmark command failed with status {status}"),
                });
            }
            results.push(started.elapsed().as_millis() as u32);
        }

        let average_ms = if results.is_empty() {
            0
        } else {
            results.iter().sum::<u32>() / results.len() as u32
        };

        Ok(CommandBenchmarkResult {
            runs_ms: results,
            average_ms,
        })
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            scenarios: default_scenarios(),
        }
    }

    pub fn scenarios(&self) -> &[ScenarioDefinition] {
        &self.scenarios
    }

    pub fn compare(
        &self,
        scenario_id: &str,
        records: &[MeasurementRecord],
    ) -> Option<ComparisonOutcome> {
        let scenario = self
            .scenarios
            .iter()
            .find(|entry| entry.id == scenario_id)?;
        let anvil = records
            .iter()
            .find(|record| {
                record.target == BenchmarkTarget::Anvil && record.scenario_id == scenario_id
            })
            .map(|record| record.value);
        let vibe_local = records
            .iter()
            .find(|record| {
                record.target == BenchmarkTarget::VibeLocal && record.scenario_id == scenario_id
            })
            .map(|record| record.value);

        let winner = match (anvil, vibe_local) {
            (Some(left), Some(right)) if left == right => None,
            (Some(left), Some(right)) => Some(match scenario.preferred_direction {
                PreferredDirection::LowerIsBetter => {
                    if left < right {
                        BenchmarkTarget::Anvil
                    } else {
                        BenchmarkTarget::VibeLocal
                    }
                }
                PreferredDirection::HigherIsBetter => {
                    if left > right {
                        BenchmarkTarget::Anvil
                    } else {
                        BenchmarkTarget::VibeLocal
                    }
                }
            }),
            _ => None,
        };

        Some(ComparisonOutcome {
            scenario_id: scenario.id.to_string(),
            axis: scenario.axis,
            winner,
            anvil_value: anvil,
            vibe_local_value: vibe_local,
        })
    }

    pub fn render_markdown_summary(&self, records: &[MeasurementRecord]) -> String {
        let mut lines = vec![
            "# Competitive Validation Summary".to_string(),
            String::new(),
            "| Scenario | Axis | Source | Anvil | vibe-local | Winner |".to_string(),
            "| --- | --- | --- | ---: | ---: | --- |".to_string(),
        ];

        for scenario in self.scenarios() {
            let outcome = self
                .compare(scenario.id, records)
                .expect("registered scenario should compare");
            let winner = match outcome.winner {
                Some(BenchmarkTarget::Anvil) => "Anvil",
                Some(BenchmarkTarget::VibeLocal) => "vibe-local",
                None => "Tie/Unknown",
            };
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} |",
                scenario.title,
                axis_label(scenario.axis),
                outcome
                    .anvil_value
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                outcome
                    .vibe_local_value
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                winner,
                source_label(records, scenario.id)
            ));
        }

        lines.join("\n")
    }
}

fn default_scenarios() -> Vec<ScenarioDefinition> {
    vec![
        ScenarioDefinition {
            id: "startup_latency_ms",
            axis: ComparisonAxis::FirstUseExperience,
            title: "Startup latency",
            metric_name: "startup_latency_ms",
            measurement: MeasurementKind::DurationMs,
            preferred_direction: PreferredDirection::LowerIsBetter,
        },
        ScenarioDefinition {
            id: "first_prompt_latency_ms",
            axis: ComparisonAxis::IterationSpeed,
            title: "First prompt latency",
            metric_name: "first_prompt_latency_ms",
            measurement: MeasurementKind::DurationMs,
            preferred_direction: PreferredDirection::LowerIsBetter,
        },
        ScenarioDefinition {
            id: "interrupt_recovery_score",
            axis: ComparisonAxis::StabilityAndRecovery,
            title: "Interrupt recovery score",
            metric_name: "interrupt_recovery_score",
            measurement: MeasurementKind::Score5,
            preferred_direction: PreferredDirection::HigherIsBetter,
        },
        ScenarioDefinition {
            id: "long_session_resume_score",
            axis: ComparisonAxis::LongSessionUsability,
            title: "Long-session resume score",
            metric_name: "long_session_resume_score",
            measurement: MeasurementKind::Score5,
            preferred_direction: PreferredDirection::HigherIsBetter,
        },
        ScenarioDefinition {
            id: "ux_clarity_score",
            axis: ComparisonAxis::UxClarity,
            title: "UX clarity score",
            metric_name: "ux_clarity_score",
            measurement: MeasurementKind::Score5,
            preferred_direction: PreferredDirection::HigherIsBetter,
        },
    ]
}

fn axis_label(axis: ComparisonAxis) -> &'static str {
    match axis {
        ComparisonAxis::FirstUseExperience => "FirstUseExperience",
        ComparisonAxis::IterationSpeed => "IterationSpeed",
        ComparisonAxis::StabilityAndRecovery => "StabilityAndRecovery",
        ComparisonAxis::LongSessionUsability => "LongSessionUsability",
        ComparisonAxis::UxClarity => "UxClarity",
    }
}

fn source_label(records: &[MeasurementRecord], scenario_id: &str) -> &'static str {
    match records.iter().find(|record| record.scenario_id == scenario_id) {
        Some(record) => match record.source {
            MeasurementSource::Measured => "Measured",
            MeasurementSource::OperationalScore => "OperationalScore",
        },
        None => "-",
    }
}
