use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

#[derive(Default, Clone)]
pub struct DrainToken(CancellationToken);

impl DrainToken {
    pub fn new() -> Self {
        DrainToken(CancellationToken::new())
    }

    /// Activates drain mode. After this, `is_draining()` returns `true`
    pub fn start_drain(&self) {
        self.0.cancel();
    }

    /// Resolves once `start_drain` was called
    pub fn drain_triggered(&self) -> WaitForCancellationFuture<'_> {
        self.0.cancelled()
    }

    /// Returns `true` if the drain mode has been activated.
    pub fn is_draining(&self) -> bool {
        self.0.is_cancelled()
    }
}
