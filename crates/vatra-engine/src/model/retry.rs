use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::{Completion, ContextWindow, ModelDriver, ModelError, ModelRequest};

/// The pauses before each retry, so five attempts in all.
const PAUSES: [Duration; 4] =
    [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8)];

/// Retries rate limits and server errors, but only while no text has reached the caller.
/// A retry after streamed text would show that text twice.
pub fn with_retries(driver: Arc<dyn ModelDriver>) -> Arc<dyn ModelDriver> {
    Arc::new(Retrying { driver })
}

struct Retrying {
    driver: Arc<dyn ModelDriver>,
}

#[async_trait]
impl ModelDriver for Retrying {
    async fn stream(
        &self,
        request: &ModelRequest,
        on_text: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<Completion, ModelError> {
        let mut attempt = 0;
        loop {
            let mut sent = false;
            let result = self
                .driver
                .stream(request, &mut |text: &str| {
                    sent = true;
                    on_text(text);
                })
                .await;
            let error = match result {
                Ok(completion) => return Ok(completion),
                Err(error) => error,
            };
            let retryable = matches!(error, ModelError::RateLimited { .. } | ModelError::Server(_));
            if sent || !retryable || attempt == PAUSES.len() {
                return Err(error);
            }
            let pause = match &error {
                ModelError::RateLimited { retry_after: Some(retry_after) } => *retry_after,
                _ => PAUSES[attempt],
            };
            tokio::time::sleep(pause).await;
            attempt += 1;
        }
    }

    fn context_window(&self) -> ContextWindow {
        self.driver.context_window()
    }

    fn supports_images(&self) -> bool {
        self.driver.supports_images()
    }
}
