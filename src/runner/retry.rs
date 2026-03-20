use std::time::Duration;

/// Calculate the delay before the next retry attempt using exponential backoff.
/// base_delay_secs * 2^(attempt - 1), capped at 3600 seconds (1 hour).
pub fn retry_delay_secs(base_delay_secs: i32, attempt: i32) -> u64 {
    let base = base_delay_secs.max(1) as u64;
    let exponent = (attempt - 1).max(0) as u32;
    let delay = base.saturating_mul(2u64.saturating_pow(exponent));
    // Cap at 1 hour
    delay.min(3600)
}

/// Determine whether a task should be retried based on its configuration
/// and the current attempt number.
pub fn should_retry(max_retries: i32, current_attempt: i32) -> bool {
    max_retries > 0 && current_attempt < max_retries + 1
}

/// Sleep for the computed backoff duration.
pub async fn sleep_with_backoff(base_delay_secs: i32, attempt: i32) {
    let secs = retry_delay_secs(base_delay_secs, attempt);
    tracing::info!("Sleeping for {}s before retry attempt", secs);
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_delay_exponential() {
        assert_eq!(retry_delay_secs(5, 1), 5);   // 5 * 2^0 = 5
        assert_eq!(retry_delay_secs(5, 2), 10);  // 5 * 2^1 = 10
        assert_eq!(retry_delay_secs(5, 3), 20);  // 5 * 2^2 = 20
        assert_eq!(retry_delay_secs(5, 4), 40);  // 5 * 2^3 = 40
    }

    #[test]
    fn test_retry_delay_capped() {
        assert_eq!(retry_delay_secs(1000, 5), 3600);
    }

    #[test]
    fn test_should_retry() {
        // max_retries=3 means we allow attempts 1,2,3 then give up at 4
        assert!(should_retry(3, 1));
        assert!(should_retry(3, 2));
        assert!(should_retry(3, 3));
        assert!(!should_retry(3, 4));
        assert!(!should_retry(0, 1));
    }
}
