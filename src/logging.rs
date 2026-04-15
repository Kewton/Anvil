use tracing_subscriber::EnvFilter;

pub fn init_logging(debug: bool) -> Result<(), String> {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|err| format!("failed to initialize logging: {err}"))
}
