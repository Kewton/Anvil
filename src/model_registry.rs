#[cfg(target_os = "macos")]
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModels {
    pub main: String,
    pub sidecar: Option<String>,
}

pub fn select_models(
    requested_main: Option<String>,
    requested_sidecar: Option<String>,
    available: &[String],
    total_memory_gib: Option<u64>,
) -> RuntimeModels {
    let main = requested_main.unwrap_or_else(|| {
        choose_model(
            available,
            total_memory_gib,
            &[
                (96, &["gpt-oss:120b", "qwen3-coder:30b", "qwen3:32b"]),
                (32, &["qwen3-coder:30b", "qwen3:14b", "qwen3:8b"]),
                (16, &["qwen3:8b", "qwen2.5-coder:7b", "qwen3:4b"]),
                (0, &["qwen3:1.7b", "qwen2.5-coder:3b"]),
            ],
        )
        .unwrap_or_else(|| {
            available
                .first()
                .cloned()
                .unwrap_or_else(|| "qwen3:8b".to_string())
        })
    });

    let sidecar = requested_sidecar.or_else(|| {
        choose_model(
            available,
            total_memory_gib,
            &[
                (96, &["qwen3-coder:30b", "qwen3:8b", "qwen3:4b"]),
                (32, &["qwen3:8b", "qwen3:4b", "qwen3:1.7b"]),
                (16, &["qwen3:1.7b", "qwen2.5-coder:1.5b"]),
                (0, &[]),
            ],
        )
    });
    let sidecar = sidecar.filter(|candidate| candidate != &main);

    RuntimeModels { main, sidecar }
}

fn choose_model(
    available: &[String],
    total_memory_gib: Option<u64>,
    tiers: &[(u64, &[&str])],
) -> Option<String> {
    let memory = total_memory_gib.unwrap_or(16);
    for (threshold, candidates) in tiers {
        if memory >= *threshold {
            for candidate in *candidates {
                if let Some(found) = find_model(available, candidate) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn find_model(available: &[String], needle: &str) -> Option<String> {
    if available.is_empty() {
        return Some(needle.to_string());
    }

    available
        .iter()
        .find(|name| *name == needle)
        .cloned()
        .or_else(|| {
            let base = needle.split(':').next().unwrap_or(needle);
            available
                .iter()
                .find(|name| name.split(':').next() == Some(base))
                .cloned()
        })
}

pub fn detect_total_memory_gib() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()?;
        return Some(bytes / 1024 / 1024 / 1024);
    }

    #[cfg(target_os = "linux")]
    {
        let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = contents
            .lines()
            .find(|line| line.starts_with("MemTotal:"))?;
        let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        return Some(kib / 1024 / 1024);
    }

    #[allow(unreachable_code)]
    None
}
