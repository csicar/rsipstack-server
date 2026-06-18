use tokio_util::sync::CancellationToken;

#[derive(Default, Clone)]
pub struct DrainToken(CancellationToken);

impl DrainToken {
    pub fn new() -> Self {
        DrainToken(CancellationToken::new())
    }

    /// Activates drain mode. After this, `is_draining()` returns `true` and `draining()` resolves immediately.
    pub fn start_drain(&self) {
        self.0.cancel();
    }

    /// Returns `true` if the drain mode has been activated.
    pub fn is_draining(&self) -> bool {
        self.0.is_cancelled()
    }
}
