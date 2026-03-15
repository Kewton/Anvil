fn main() {
    if let Err(err) = anvil::app::run() {
        eprintln!("anvil error: {err}");
        eprintln!();
        eprintln!("{}", anvil::app::error_guidance(&err));
        std::process::exit(1);
    }
}
