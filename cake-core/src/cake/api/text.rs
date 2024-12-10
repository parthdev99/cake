use crate::cake::Master;
use crate::models::chat::Message;
use crate::models::{ImageGenerator, TextGenerator};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use tokio::sync::{RwLock}; // Import mpsc for channels
use async_stream::stream; // Import stream macro from async-stream crate
use futures::stream::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

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
    // Create an unbounded channel for streaming
    let (tx, rx): (UnboundedSender<String>, UnboundedReceiver<String>) = tokio::sync::mpsc::unbounded_channel();

    // Clone the state and messages for the async block
    let state_clone = state.clone();
    let messages_clone = messages.0.clone();

    // Spawn a task to generate text and send dummy data every 30 seconds
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
            if let Err(e) = tx.send(token) {
                log::error!("Failed to send token: {}", e);
            }
        }).await.unwrap();

        // Send dummy data every 30 seconds
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Err(e) = tx.send("Dummy message every 30 seconds".to_string()) {
                log::error!("Failed to send dummy message: {}", e);
                break; // Exit loop if sending fails
            }
        }
    });

    // Create a stream from the unbounded receiver
    let rx_stream = UnboundedReceiverStream::new(rx);
    let event_stream = rx_stream.map(|token| {
        Ok::<actix_web::web::Bytes, actix_web::Error>(actix_web::web::Bytes::from(format!("data: {}\n\n", token)))
    });

    // Return streaming response
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("X-Accel-Buffering", "no"))
        .streaming(Box::pin(event_stream))
}
