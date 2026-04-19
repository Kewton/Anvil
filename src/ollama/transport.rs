use reqwest::blocking::{Client, Response};
use serde::Serialize;

#[derive(Serialize)]
struct RequestOptions {
    temperature: f32,
    num_ctx: usize,
    num_predict: usize,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    raw: bool,
    stream: bool,
    keep_alive: i32,
    options: RequestOptions,
}

pub(crate) struct GenerateTransport<'a> {
    base_url: &'a str,
    http: &'a Client,
    context_window: usize,
    max_predict: usize,
}

impl<'a> GenerateTransport<'a> {
    pub(crate) fn new(
        base_url: &'a str,
        http: &'a Client,
        context_window: usize,
        max_predict: usize,
    ) -> Self {
        Self {
            base_url,
            http,
            context_window,
            max_predict,
        }
    }

    pub(crate) fn send_generate_request(
        &self,
        model: &str,
        prompt: &str,
        stream: bool,
        temperature: f32,
    ) -> Result<Response, reqwest::Error> {
        let request = GenerateRequest {
            model,
            prompt,
            raw: true,
            stream,
            keep_alive: -1,
            options: self.request_options(temperature),
        };
        self.http
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
    }

    fn request_options(&self, temperature: f32) -> RequestOptions {
        RequestOptions {
            temperature,
            num_ctx: self.context_window,
            num_predict: self.max_predict,
        }
    }
}

pub fn should_use_native_tool_calls(_model: &str) -> bool {
    false
}
