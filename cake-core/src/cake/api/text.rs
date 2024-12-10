use crate::cake::Master;
use crate::models::chat::Message;
use crate::models::{ImageGenerator, TextGenerator};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, mpsc}; // Import mpsc for channels
use async_stream::stream; // Import stream macro from async-stream crate
use futures::stream; // Import stream for creating streams

#[derive(Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
}

#[derive(Serialize)]
struct Choice {
    pub index: usize,
    pub message: Message,
}

#[derive(Serialize)]
struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
}

impl ChatResponse {
    pub fn from_assistant_response(model: String, message: String) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let object = String::from("chat.completion");
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let choices = vec![Choice {
            index: 0,
            message: Message::assistant(message),
        }];
        Self {
            id,
            object,
            created,
            model,
            choices,
        }
    }
}
pub async fn generate_text<TG, IG>(
    state: web::Data<Arc<RwLock<Master<TG, IG>>>>,
    messages: web::Json<ChatRequest>,
) -> impl Responder
where
    TG: TextGenerator + Send + Sync + 'static,
    IG: ImageGenerator + Send + Sync + 'static,
{
    // Create a streaming response
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("X-Accel-Buffering", "no")
        .streaming(async_stream::stream! {
            let state = state.clone();
            let messages = messages.0.clone();

            let mut master = state.write().await;
            master.reset().unwrap();
            let llm_model = master.llm_model.as_mut().expect("LLM model not found");

            // Add messages to the model
            for message in messages.messages {
                llm_model.add_message(message).unwrap();
            }

            // Generate text with streaming
            master.generate_text(|data| {
                let token = data.to_string();

                println!("Generated token: {}", token);
                yield Ok(actix_web::web::Bytes::from(format!("data: {}\n\n", token)));
            }).await.unwrap();
        })
}
