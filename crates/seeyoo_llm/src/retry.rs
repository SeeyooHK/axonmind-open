use crate::errors::LlmError;
use crate::types::RetryConfig;
use std::time::Duration;
use tokio::time::sleep;

pub async fn retry_operation<F, Fut, T>(
    config: &RetryConfig,
    mut operation: F,
) -> Result<T, LlmError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, LlmError>>,
{
    let mut attempts = 0;

    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                if !err.is_retryable() || attempts >= config.max_retries {
                    return Err(err);
                }

                attempts += 1;

                let exponent = (attempts - 1) as u32;
                let base_delay_ms = (config.initial_delay_ms as f64
                    * (config.backoff_multiplier as f64).powi(exponent as i32))
                    as u64;

                let backoff_delay = std::cmp::min(base_delay_ms, config.max_delay_ms);

                let jitter = 0.8 + (rand::random::<f64>() * 0.4);
                let jitter_delay = (backoff_delay as f64 * jitter) as u64;

                let wait_ms = match &err {
                    LlmError::RateLimited {
                        retry_after_ms: Some(ms),
                        ..
                    } => std::cmp::max(jitter_delay, *ms),
                    _ => jitter_delay,
                };

                tracing::warn!(
                    "Operation failed (attempt {}/{}): {:?}. Retrying in {}ms...",
                    attempts,
                    config.max_retries,
                    err,
                    wait_ms
                );

                sleep(Duration::from_millis(wait_ms)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn test_retry_success_after_failure() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 1,
            max_delay_ms: 10,
            backoff_multiplier: 2.0,
        };
        let attempts = Arc::new(Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_operation(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                let mut count = attempts.lock().await;
                *count += 1;
                if *count < 2 {
                    Err(LlmError::RequestFailed("transient".into()))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*attempts.lock().await, 2);
    }

    #[tokio::test]
    async fn test_retry_max_attempts_exceeded() {
        let config = RetryConfig {
            max_retries: 2,
            initial_delay_ms: 1,
            max_delay_ms: 10,
            backoff_multiplier: 2.0,
        };
        let attempts = Arc::new(Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<(), LlmError> = retry_operation(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                let mut count = attempts.lock().await;
                *count += 1;
                Err(LlmError::RequestFailed("persistent".into()))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(*attempts.lock().await, 3);
    }

    #[tokio::test]
    async fn test_retry_respects_rate_limit_header() {
        let config = RetryConfig {
            max_retries: 2,
            initial_delay_ms: 1,
            max_delay_ms: 100,
            backoff_multiplier: 2.0,
        };
        let attempts = Arc::new(Mutex::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_operation(&config, || {
            let attempts = attempts_clone.clone();
            async move {
                let mut count = attempts.lock().await;
                *count += 1;
                if *count == 1 {
                    Err(LlmError::RateLimited {
                        provider: "test".into(),
                        retry_after_ms: Some(1),
                    })
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*attempts.lock().await, 2);
    }
}
