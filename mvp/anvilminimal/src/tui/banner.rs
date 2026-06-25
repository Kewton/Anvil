use std::io::{self, IsTerminal};

use crate::config::{Config, Provider};

const MIN_BANNER_WIDTH: u16 = 50;
const NEON_GRADIENT_256: [u8; 5] = [51, 39, 93, 201, 21];
const ANVIL_ASCII_ART: [&str; 5] = [
    "                                          ",
    "  ╔═╗ ╔╗╔ ╦  ╦ ╦ ╦                        ",
    "  ╠═╣ ║║║ ╚╗╔╝ ║ ║     local-first agent  ",
    "  ╩ ╩ ╝╚╝  ╚╝  ╩ ╩═╝                      ",
    "                                          ",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerStyle {
    Neon,
    MonoArt,
    Legacy4Line,
}

pub fn print_startup_banner(config: &Config) -> anyhow::Result<()> {
    let tty = io::stdout().is_terminal();
    let width = if tty {
        crossterm::terminal::size().ok().map(|(width, _)| width)
    } else {
        None
    };
    let style = decide_banner_style(tty, crate::tui::terminal::no_color(), width);
    print!("{}", render_startup_banner(config, style));
    Ok(())
}

pub fn decide_banner_style(tty: bool, no_color: bool, width: Option<u16>) -> BannerStyle {
    if !tty {
        return BannerStyle::Legacy4Line;
    }
    match width {
        None => BannerStyle::Legacy4Line,
        Some(width) if width < MIN_BANNER_WIDTH => BannerStyle::Legacy4Line,
        Some(_) if no_color => BannerStyle::MonoArt,
        Some(_) => BannerStyle::Neon,
    }
}

pub fn render_startup_banner(config: &Config, style: BannerStyle) -> String {
    let mut out = String::new();
    match style {
        BannerStyle::Neon => {
            for (index, line) in ANVIL_ASCII_ART.iter().enumerate() {
                let color = NEON_GRADIENT_256[index];
                out.push_str(&format!("\x1b[38;5;{color}m{line}\x1b[0m\n"));
            }
        }
        BannerStyle::MonoArt => {
            for line in ANVIL_ASCII_ART {
                out.push_str(line);
                out.push('\n');
            }
        }
        BannerStyle::Legacy4Line => {}
    }

    out.push_str("anvilminimal ");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push('\n');
    out.push_str(&format!(
        "model={} provider={} planner={} planner_provider={}\n",
        sanitize_banner_text(&config.model),
        provider_label(config.provider),
        sanitize_banner_text(&config.planner_model),
        provider_label(config.planner_provider),
    ));
    out.push_str(&format!(
        "mode=Act cwd={}\n",
        sanitize_banner_text(&config.workspace_root.display().to_string())
    ));
    out.push_str(&format!(
        "context_budget={} yes={}\n",
        config.context_budget, config.yes
    ));
    out
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Ollama => "ollama",
        Provider::Openai => "openai",
        Provider::Gemini => "gemini",
    }
}

fn sanitize_banner_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            workspace_root: std::path::PathBuf::from("/tmp/anvilminimal"),
            state_dir: std::path::PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 65536,
            model: "m\x1b[31m".to_string(),
            provider: Provider::Ollama,
            planner_model: "pm".to_string(),
            planner_provider: Provider::Gemini,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    #[test]
    fn banner_style_wide_tty_uses_art() {
        assert_eq!(
            decide_banner_style(true, false, Some(80)),
            BannerStyle::Neon
        );
        assert_eq!(
            decide_banner_style(true, true, Some(80)),
            BannerStyle::MonoArt
        );
    }

    #[test]
    fn banner_style_non_tty_or_narrow_is_legacy() {
        assert_eq!(
            decide_banner_style(false, false, Some(80)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(true, false, Some(20)),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn banner_mono_includes_ascii_art_lines() {
        let out = render_startup_banner(&config(), BannerStyle::MonoArt);
        for line in ANVIL_ASCII_ART {
            assert!(out.contains(line));
        }
        assert!(!out.contains("\x1b[31m"));
    }

    #[test]
    fn banner_legacy_has_dynamic_lines_without_art() {
        let out = render_startup_banner(&config(), BannerStyle::Legacy4Line);
        assert!(out.contains("anvilminimal"));
        assert!(out.contains("context_budget=65536 yes=true"));
        assert!(!out.contains("╔═╗"));
    }
}
