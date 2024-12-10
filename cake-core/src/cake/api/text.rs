use crate::cake::Master;
use crate::models::chat::Message;
use crate::models::{ImageGenerator, TextGenerator};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, mpsc}; // Import mpsc for channels
use async_stream::stream; // Import stream macro from async-stream crate
use futures::stream::{Stream, StreamExt};
use std::pin::Pin;

#[derive(Deserialize, Clone)]
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
    // Create a channel for streaming
    let (tx, rx) = tokio::sync::mpsc::channel(100);

    // Clone the state and messages for the async block
    let state_clone = state.clone();
    let messages_clone = messages.0.clone();

    // Spawn a task to generate text
    tokio::spawn(async move {
        let mut master = state_clone.write().await;
        master.reset().unwrap();
        let llm_model = master.llm_model.as_mut().expect("LLM model not found");

        // Add messages to the model
        for message in messages_clone.messages {
            llm_model.add_message(message).unwrap();
        }

        // Generate text with a streaming sender
        master.generate_text(|data| {
            let token = data.to_string();
            let tx_clone = tx.clone();
            println!("Generated token: {}", token);
            tokio::spawn(async move {
                if let Err(e) = tx_clone.send(token).await {
                    log::error!("Failed to send token: {}", e);
                }
            });
        }).await.unwrap();

        // Signal end of stream
        drop(tx);
    });

    // Create a streaming response
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("X-Accel-Buffering", "no"))
        .streaming(create_stream(rx))
}

// Custom stream creation function
fn create_stream(
    rx: tokio::sync::mpsc::Receiver<String>,
) -> Pin<Box<dyn Stream<Item = Result<actix_web::web::Bytes, actix_web::Error>>>> {
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(token) => {
                let bytes = actix_web::web::Bytes::from(format!("data: {}\n\n", token));
                Some((Ok(bytes), rx))
            }
            None => None,
        }
    }))
}
