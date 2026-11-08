//! Finding local models: hardware, Hugging Face, GGUF headers, memory estimates, labels and suggestions.
//! Nothing here starts processes or writes files; the service does that.

pub mod gguf;
pub mod hardware;
pub mod llama;
pub mod variant;
pub mod estimate;
pub mod hf;
pub mod label;
pub mod suggest;
