//! The Tokio runtime that does all network and keyring I/O.
//!
//! GTK owns the main thread. Anything that can block (HTTP, D-Bus, disk) runs
//! on this runtime's worker threads and is awaited from the GLib main context
//! with [`spawn`], so the UI never waits on the network.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

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
pub async fn spawn<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match runtime().spawn(future).await {
        Ok(output) => output,
        Err(err) => std::panic::resume_unwind(err.into_panic()),
    }
}
