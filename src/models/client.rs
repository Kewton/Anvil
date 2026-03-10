pub trait ModelClient {
    fn provider_name(&self) -> &'static str;
}
