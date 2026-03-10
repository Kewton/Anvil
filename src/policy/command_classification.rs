#[derive(Debug, Clone, Copy)]
pub enum CommandClass {
    SafeRead,
    LocalValidation,
    Networked,
    Destructive,
}
