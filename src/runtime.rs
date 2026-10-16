//! The tokio runtime the S3 SDK runs on, bridged into the GTK main loop.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("failed to start tokio runtime")
    })
}

/// Runs `fut` on the tokio runtime. Await the result from GTK code (`glib::spawn_future_local`).
pub async fn bg<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    runtime()
        .spawn(fut)
        .await
        .expect("background task panicked")
}
