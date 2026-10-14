use futures_util::task::AtomicWaker;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
pub(crate) struct AttentionState {
    pub(crate) requested: AtomicBool,
    pub(crate) armed: AtomicBool,
    pub(crate) waker: AtomicWaker,
}

/// Cancellation request for one execution on the original TDS connection.
/// The connection sends ATTENTION through its existing TLS transport while
/// being polled, then drains the response through DONE_ATTN before reuse.
#[derive(Clone)]
pub struct AttentionHandle(pub(crate) Arc<AttentionState>);

impl std::fmt::Debug for AttentionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttentionHandle").finish_non_exhaustive()
    }
}

impl AttentionHandle {
    pub(crate) fn new() -> Self {
        let state = Arc::new(AttentionState::default());
        state.armed.store(true, Ordering::SeqCst);
        Self(state)
    }

    /// Queue ATTENTION and wake the original query. This is a request, not
    /// server confirmation; the query returns Error::Cancelled after DONE_ATTN.
    pub fn request(&self) -> bool {
        if !self.0.armed.load(Ordering::SeqCst) {
            return false;
        }
        self.0.requested.store(true, Ordering::SeqCst);
        self.0.waker.wake();
        true
    }

    /// Disable a completed execution's still-unsent request.
    pub fn disarm(&self) {
        self.0.armed.store(false, Ordering::SeqCst);
        self.0.requested.store(false, Ordering::SeqCst);
        self.0.waker.wake();
    }
}
