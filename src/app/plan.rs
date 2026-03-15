//! Plan management methods extracted from the main app module.

use crate::contracts::AppEvent;
use crate::session::{MessageRole, SessionMessage};

use super::render;
use super::{App, AppError};

impl App {
    pub(crate) fn add_plan_item(&mut self, item: String) -> Result<String, AppError> {
        let mut items = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .map(|plan| plan.items.clone())
            .unwrap_or_default();
        items.push(item);
        let active_index = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .and_then(|plan| plan.active_index)
            .or(Some(0));
        self.update_plan_snapshot(items, active_index, AppEvent::PlanItemAdded)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    pub(crate) fn focus_plan_item(&mut self, index: usize) -> Result<String, AppError> {
        let items = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .map(|plan| plan.items.clone())
            .unwrap_or_default();
        if items.is_empty() {
            return Ok("[A] anvil > plan\n  no active plan".to_string());
        }
        let active_index = Some(index.min(items.len().saturating_sub(1)));
        self.update_plan_snapshot(items, active_index, AppEvent::PlanFocusChanged)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    pub(crate) fn clear_plan_items(&mut self) -> Result<String, AppError> {
        self.update_plan_snapshot(Vec::new(), None, AppEvent::PlanCleared)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    pub(crate) fn save_plan_checkpoint(&mut self, note: String) -> Result<String, AppError> {
        let checkpoint = SessionMessage::new(
            MessageRole::System,
            "anvil",
            format!("[plan checkpoint] {note}"),
        )
        .with_id(self.next_message_id("checkpoint"));
        self.session.push_message(checkpoint);
        self.persist_session(AppEvent::PlanCheckpointSaved)?;
        Ok(format!("[A] anvil > checkpoint saved\n  {note}"))
    }

    pub(crate) fn update_plan_snapshot(
        &mut self,
        items: Vec<String>,
        active_index: Option<usize>,
        event: AppEvent,
    ) -> Result<(), AppError> {
        let mut snapshot = self.state_machine.snapshot().clone();
        snapshot.plan = if items.is_empty() {
            None
        } else {
            Some(crate::contracts::PlanView {
                items,
                active_index,
            })
        };
        snapshot.last_event = Some(event);
        self.state_machine.replace_snapshot(snapshot.clone());
        self.session.set_last_snapshot(snapshot);
        self.persist_session(event)
    }
}
