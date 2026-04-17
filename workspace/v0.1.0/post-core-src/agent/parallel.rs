use std::thread;

use crate::ollama::client::OllamaClient;
use crate::session::store::ConversationMessage;

pub fn run_parallel_analysis(
    client: OllamaClient,
    model: String,
    tasks: Vec<String>,
) -> Result<Vec<String>, String> {
    if tasks.len() < 2 || tasks.len() > 4 {
        return Err("parallel analysis requires 2 to 4 tasks".to_string());
    }

    let mut handles = Vec::new();
    for task in tasks {
        let client = client.clone();
        let model = model.clone();
        handles.push(thread::spawn(move || {
            let messages = vec![
                ConversationMessage::system(
                    "You are a read-only analysis worker. Answer directly without using tools. Keep the answer concise and factual.".to_string(),
                ),
                ConversationMessage::user(task),
            ];
            client.chat_text(&model, &messages).map(|reply| reply.content)
        }));
    }

    let mut outputs = Vec::new();
    for handle in handles {
        let output = handle
            .join()
            .map_err(|_| "parallel worker panicked".to_string())??;
        outputs.push(output);
    }
    Ok(outputs)
}
