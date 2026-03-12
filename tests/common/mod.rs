use anvil::app::App;
use anvil::config::EffectiveConfig;
use anvil::provider::ProviderRuntimeContext;

pub fn build_app() -> App {
    let config = EffectiveConfig::load().expect("config should load");
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    App::new(config, provider).expect("app should initialize")
}
