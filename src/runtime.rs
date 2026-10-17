//! The Tokio runtime that does all network and keyring I/O.
//!
//! GTK owns the main thread. Anything that can block (HTTP, D-Bus, disk) runs
//! on this runtime's worker threads and is awaited from the GLib main context
//! with [`spawn`], so the UI never waits on the network.

use std::future::Future;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use tokio::runtime::Runtime;
use tokio::task::AbortHandle;

pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("swell-io")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// Runs `future` on the I/O runtime and resolves with its output on whatever
/// executor awaits it (usually `glib::spawn_future_local`).
///
/// Dropping the returned future (for example by aborting the GLib task that
/// awaits it) aborts the work on the runtime too. A panic in the task comes
/// back as an error instead of unwinding through the GLib main loop.
pub async fn spawn<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().spawn(future);
    let _abort = AbortOnDrop(handle.abort_handle());
    match handle.await {
        Ok(output) => output,
        Err(err) if err.is_panic() => Err(anyhow!("background task panicked")),
        Err(err) => Err(anyhow!("background task cancelled: {err}")),
    }
}

/// Aborts a runtime task when dropped (a no-op once it has finished).
pub struct AbortOnDrop(pub AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
