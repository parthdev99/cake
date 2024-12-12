use std::io::Write;

use crate::models::{chat::Message, ImageGenerator, TextGenerator};

use super::{api, Context};

use crate::{ImageGenerationArgs, ModelType};
use anyhow::Result;
use image::{ImageBuffer, Rgb};

/// A master connects to, communicates with and orchestrates the workers.
pub struct Master<TG, IG> {
    pub ctx: Context,
    pub llm_model: Option<Box<TG>>,
    pub sd_model: Option<Box<IG>>,
}

impl<TG: TextGenerator + Send + Sync + 'static, IG: ImageGenerator + Send + Sync + 'static>
    Master<TG, IG>
{
    /// Create a new instance.
    pub async fn new(mut ctx: Context) -> Result<Self> {
        match ctx.args.model_type {
            ModelType::ImageModel => {
                let sd_model = IG::load(&mut ctx).await?;
                Ok(Self {
                    ctx,
                    sd_model,
                    llm_model: None,
                })
            }
            ModelType::TextModel => {
                let llm_model = TG::load(&mut ctx).await?;
                Ok(Self {
                    ctx,
                    llm_model,
                    sd_model: None,
                })
            }
        }
    }

    pub async fn run(mut self) -> Result<()> {
        if self.ctx.args.api.is_some() {
            // run as REST api
            api::start(self).await?;
        } else {
            // if running in cli mode, pre add system and user prompts
            if self.ctx.args.model_type == ModelType::TextModel {
                let llm_model = self.llm_model.as_mut().expect("LLM model not found");
                llm_model.add_message(Message::system(self.ctx.args.system_prompt.clone()))?;
                llm_model.add_message(Message::user(self.ctx.args.prompt.clone()))?;

                // just run one generation to stdout
                self.generate_text(|data| {
                    if data.is_empty() {
                        println!();
                    } else {
                        print!("{data}")
                    }
                    std::io::stdout().flush().unwrap();
                })
                .await?;
            } else {
                let mut step_num = 0;

                self.generate_image(self.ctx.args.sd_img_gen_args.clone(), move |images| {
                    let mut batched_num = 0;
                    for image in images {
                        image
                            .save(format!("images/image_{}_{}.png", batched_num, step_num))
                            .expect("Error saving image to disk");
                        batched_num += 1;
                    }
                    step_num += 1;
                })
                .await?;
            }
        }

        Ok(())
    }

    /// Reset the master state for a new inference.
    pub fn reset(&mut self) -> Result<()> {
        self.llm_model
            .as_mut()
            .expect("LLM model not found")
            .reset()
    }

    /// clear worker kv cache
    pub async fn goodbye(&mut self) -> Result<()> {
        self.llm_model
            .as_mut()
            .expect("LLM model not found")
            .goodbye()
            .await
    }

    /// Start the generation loop and call the stream function for every token.
    pub async fn generate_text<S>(&mut self, mut stream: S) -> Result<()>
    where
        S: FnMut(&str),
    {
        log::info!(
            "starting the inference loop (mem={})\n\n",
            human_bytes::human_bytes(memory_stats::memory_stats().unwrap().physical_mem as f64)
        );
    
        log::debug!("  ctx.args.sample_len = {}", self.ctx.args.sample_len);
    
        let llm_model = self.llm_model.as_mut().expect("LLM model not found");
    
        // Start timing for prefill (time to first token)
        let prefill_start = std::time::Instant::now();
    
        // Generate the first token
        let first_token = llm_model.next_token(0).await?;
        if first_token.is_end_of_stream {
            return Ok(()); // Exit if end of stream is reached
        } else {
            stream(&first_token.to_string());
        }
    
        // Record the time taken for prefill (first token generation)
        let prefill_duration = prefill_start.elapsed();
    
        // Start timing for token generation
        let mut start_gen = std::time::Instant::now();
        
        let mut generated_count = 1; // Start with 1 for the first generated token
    
        for index in 1..self.ctx.args.sample_len { // Start from 1 since 0 was already generated
            let token = llm_model.next_token(index).await?;
            if token.is_end_of_stream {
                break; // Exit loop if end of stream is reached
            } else {
                stream(&token.to_string());
                generated_count += 1; // Increment generated token count
            }
        }
    
        // Signal end of stream
        stream("");
    
        let dt = start_gen.elapsed();
        
        // Calculate throughput for generated tokens
        let generation_throughput = (generated_count - 1) as f64 / dt.as_secs_f64(); // tokens per second
    
        log::info!(
            "{} tokens generated ({} token/s) - mem={}",
            generated_count,
            generation_throughput,
            human_bytes::human_bytes(memory_stats::memory_stats().unwrap().physical_mem as f64)
        );
    
        // Log prefill duration
        log::info!(
            "Prefill duration: {:?} ",
            prefill_duration,
        );
    
        Ok(())
    }

    pub async fn generate_image<F>(&mut self, args: ImageGenerationArgs, callback: F) -> Result<()>
    where
        F: FnMut(Vec<ImageBuffer<Rgb<u8>, Vec<u8>>>) + Send + 'static,
    {
        let sd_model = self.sd_model.as_mut().expect("SD model not found");
        sd_model.generate_image(&args, callback).await
    }
}
