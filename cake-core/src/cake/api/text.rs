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

// Ensure all necessary traits are imported
use std::fmt::Debug;
use thiserror::Error;

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

// Custom error type for better error handling
#[derive(Error, Debug)]
enum StreamingError {
    #[error("Failed to reset model")]
    ModelResetError,
    #[error("LLM model not found")]
    ModelNotFoundError,
    #[error("Failed to add message")]
    MessageAddError,
    #[error("Text generation failed")]
    GenerationError,
    #[error("Channel send failed")]
    ChannelSendError,
}

pub async fn generate_text<TG, IG>(
    state: web::Data<Arc<RwLock<Master<TG, IG>>>>,
    messages: web::Json<ChatRequest>,
) -> impl Responder 
where
    TG: TextGenerator + Send + Sync + 'static,
    IG: ImageGenerator + Send + Sync + 'static,
{
    // Create a channel with a larger buffer
    let (tx, rx) = tokio::sync::mpsc::channel(1000);

    // Clone the state and messages for the async block
    let state_clone = state.clone();
    let messages_clone = messages.0.clone();

    // Spawn a task to generate text
    tokio::spawn(async move {
        // Wrap the entire generation process in a result
        let generation_result: Result<(), StreamingError> = async {
            // Acquire write lock
            let mut master = state_clone.write().await;

            // Reset the model
            master.reset().map_err(|_| StreamingError::ModelResetError)?;

            // Ensure LLM model exists
            let llm_model = master.llm_model.as_mut()
                .ok_or(StreamingError::ModelNotFoundError)?;

            // Add messages to the model
            for message in messages_clone.messages {
                llm_model.add_message(message)
                    .map_err(|_| StreamingError::MessageAddError)?;
            }

            // Create a channel sender that can be moved into the closure
            let tx_clone = tx.clone();

            // Generate text with streaming
            master.generate_text(move |data| {
                let token = data.to_string();
                
                // Spawn a task for each token
                let tx_for_token = tx_clone.clone();
                tokio::spawn(async move {
                    if let Err(_) = tx_for_token.send(token).await {
                        // Log error silently to avoid panics
                        log::error!("Failed to send token through channel");
                    }
                });
            }).await.map_err(|_| StreamingError::GenerationError)?;

            // Explicitly indicate success
            Ok(())
        }.await;

        // Handle any errors that occurred during generation
        if let Err(e) = generation_result {
            log::error!("Text generation error: {:?}", e);
        }

        // Always drop the sender to signal stream end
        drop(tx);
    });

    // Create a streaming response
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("X-Accel-Buffering", "no"))
        .streaming(create_event_stream(rx))
}

// Create a stream of server-sent events
fn create_event_stream(
    mut rx: tokio::sync::mpsc::Receiver<String>
) -> Pin<Box<dyn Stream<Item = Result<actix_web::web::Bytes, actix_web::Error>>>> {
    Box::pin(async_stream::stream! {
        while let Some(token) = rx.recv().await {
            // Format as server-sent event
            let event = format!("data: {}\n\n", token);
            yield Ok(actix_web::web::Bytes::from(event));
        }
    })
}
