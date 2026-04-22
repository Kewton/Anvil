use anvil::modes::plan_act::{ExecutionMode, ModeState, TaskProfile};
use tempfile::tempdir;

#[test]
fn mode_state_enters_plan_and_approves() {
    let dir = tempdir().unwrap();
    let mut state = ModeState::default();
    let plan_path = state
        .enter_plan(dir.path().to_path_buf(), TaskProfile::Coding)
        .unwrap();
    assert_eq!(state.mode, ExecutionMode::Plan);
    assert_eq!(state.active_plan_path.as_deref(), Some(plan_path.as_path()));
    assert_eq!(state.task_profile, TaskProfile::Coding);
    state.approve();
    assert_eq!(state.mode, ExecutionMode::Act);
}
