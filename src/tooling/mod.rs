#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionClass {
    Safe,
    Confirm,
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    ParallelSafe,
    SequentialOnly,
}
