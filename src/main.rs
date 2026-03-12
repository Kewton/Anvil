fn main() {
    if let Err(err) = anvil::app::run() {
        eprintln!("anvil startup error: {err}");
        std::process::exit(1);
    }
}
