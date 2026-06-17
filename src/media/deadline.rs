//! Resettable deadline timer for timeout handling.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::Instant;

/// A resettable deadline timer.
///
/// # Example
///
/// ```ignore
/// let mut deadline = Deadline::new(Duration::from_secs(30));
///
/// loop {
///     tokio::select! {
///         _ = &mut deadline => {
///             println!("Timeout!");
///             break;
///         }
///         data = socket.recv() => {
///             deadline.reset(); // Reset on activity
///             // ... handle data
///         }
///     }
/// }
/// ```
pub struct Deadline {
    sleep: Pin<Box<tokio::time::Sleep>>,
    duration: Duration,
}

impl Deadline {
    /// Create a new deadline that will fire after `duration`.
    pub fn new(duration: Duration) -> Self {
        Self {
            sleep: Box::pin(tokio::time::sleep(duration)),
            duration,
        }
    }

    /// Reset the deadline to fire `duration` from now.
    pub fn reset(&mut self) {
        self.sleep.as_mut().reset(Instant::now() + self.duration);
    }
}

impl Future for Deadline {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.sleep.as_mut().poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deadline_fires() {
        let mut deadline = Deadline::new(Duration::from_millis(10));

        // Should complete after ~10ms
        (&mut deadline).await;
    }

    #[tokio::test]
    async fn test_deadline_reset() {
        let mut deadline = Deadline::new(Duration::from_millis(50));

        // Wait 30ms, then reset
        tokio::time::sleep(Duration::from_millis(30)).await;
        deadline.reset();

        // Wait another 30ms - deadline should NOT have fired yet (only 30ms since reset)
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Use tokio::select to check if deadline is ready without blocking
        tokio::select! {
            biased;
            _ = &mut deadline => {
                panic!("Deadline fired too early after reset");
            }
            _ = tokio::time::sleep(Duration::from_millis(1)) => {
                // Expected: deadline not ready yet
            }
        }

        // Wait the remaining time - deadline should fire
        tokio::time::sleep(Duration::from_millis(25)).await;
        tokio::select! {
            biased;
            _ = &mut deadline => {
                // Expected: deadline fired
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                panic!("Deadline should have fired by now");
            }
        }
    }

    #[test]
    fn test_deadline_is_unpin() {
        fn assert_unpin<T: Unpin>() {}
        assert_unpin::<Deadline>();
    }
}
