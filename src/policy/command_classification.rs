#[derive(Debug, Clone, Copy)]
pub enum CommandClass {
    SafeRead,
    LocalValidation,
    Networked,
    Destructive,
}

pub fn classify_command(program: &str, args: &[String]) -> CommandClass {
    match program {
        "pwd" | "ls" | "find" | "rg" | "cat" | "head" | "tail" | "wc" => CommandClass::SafeRead,
        "cargo"
            if matches!(
                args.first().map(String::as_str),
                Some("test" | "check" | "fmt" | "clippy")
            ) =>
        {
            CommandClass::LocalValidation
        }
        "pytest" | "ruff" | "make" => CommandClass::LocalValidation,
        "git"
            if matches!(
                args.first().map(String::as_str),
                Some("status" | "diff" | "show")
            ) =>
        {
            CommandClass::SafeRead
        }
        "git"
            if matches!(
                args.first().map(String::as_str),
                Some("clone" | "fetch" | "pull" | "push")
            ) =>
        {
            CommandClass::Networked
        }
        "git" if matches!(args.first().map(String::as_str), Some("reset" | "clean")) => {
            CommandClass::Destructive
        }
        "curl" | "wget" => CommandClass::Networked,
        "rm" => CommandClass::Destructive,
        "npm" | "pnpm" | "yarn" | "bun"
            if matches!(args.first().map(String::as_str), Some("install" | "add")) =>
        {
            CommandClass::Networked
        }
        "npm" | "pnpm" | "yarn" | "bun" => CommandClass::LocalValidation,
        _ => CommandClass::LocalValidation,
    }
}
