pub mod banner;
pub mod footer;
pub mod interrupt;
pub mod markdown;
pub mod presentation;
pub mod repl;
pub mod slash;
pub mod spinner;
pub mod status;
pub mod status_bus;
pub mod terminal;
pub mod ux_demo;

use std::sync::{Arc, Mutex};

use crate::config::Config;

use self::footer::{Footer, FreezeGuard};
use self::interrupt::{InterruptMonitor, PauseGuard};
use self::spinner::Spinner;
use self::status::UiStatus;

pub trait InteractionUi {
    fn before_model_call(&self, label: &str) -> UiGuard;
    fn before_tool_call(&self, name: &str) -> UiGuard;
    fn publish_status(&self, status: UiStatus);
    fn interrupted(&self) -> bool;
    fn force_interrupted(&self) -> bool {
        false
    }
}

pub trait OutputRenderer {
    fn render_assistant(&self, raw_text: &str) -> anyhow::Result<()>;
}

pub struct UiGuard {
    _footer: Option<FreezeGuard>,
    _spinner: Option<Spinner>,
}

impl UiGuard {
    pub fn noop() -> Self {
        Self {
            _footer: None,
            _spinner: None,
        }
    }

    fn active(footer: Option<FreezeGuard>, spinner: Option<Spinner>) -> Self {
        Self {
            _footer: footer,
            _spinner: spinner,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopUi;

pub static NOOP_UI: NoopUi = NoopUi;

impl InteractionUi for NoopUi {
    fn before_model_call(&self, _label: &str) -> UiGuard {
        UiGuard::noop()
    }

    fn before_tool_call(&self, _name: &str) -> UiGuard {
        UiGuard::noop()
    }

    fn publish_status(&self, _status: UiStatus) {}

    fn interrupted(&self) -> bool {
        false
    }
}

pub struct TerminalUi {
    footer: Footer,
    interrupt: Arc<Mutex<InterruptMonitor>>,
}

impl TerminalUi {
    pub fn new(config: &Config) -> Self {
        let footer = Footer::start(config);
        let interrupt = Arc::new(Mutex::new(InterruptMonitor::start()));
        let ui = Self { footer, interrupt };
        ui.publish_status(UiStatus::from_config(config));
        ui
    }

    pub fn pause_for_prompt(&self) -> PromptGuard {
        PromptGuard {
            _interrupt: PauseGuard::new(self.interrupt.clone()),
            _footer: self.footer.freeze(),
        }
    }

    pub fn reset_interrupt(&self) {
        if let Ok(monitor) = self.interrupt.lock() {
            monitor.reset();
        }
    }
}

impl InteractionUi for TerminalUi {
    fn before_model_call(&self, label: &str) -> UiGuard {
        UiGuard::active(None, Spinner::start(label))
    }

    fn before_tool_call(&self, name: &str) -> UiGuard {
        UiGuard::active(None, Spinner::start(format!("tool {name}")))
    }

    fn publish_status(&self, status: UiStatus) {
        self.footer.publish(status);
    }

    fn interrupted(&self) -> bool {
        self.interrupt
            .lock()
            .map(|monitor| monitor.interrupted())
            .unwrap_or(false)
    }

    fn force_interrupted(&self) -> bool {
        self.interrupt
            .lock()
            .map(|monitor| monitor.force_interrupted())
            .unwrap_or(false)
    }
}

pub struct PromptGuard {
    _interrupt: Option<PauseGuard>,
    _footer: Option<FreezeGuard>,
}
