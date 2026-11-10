//! The tokio runtime that HTTP requests and the local server run on.
//!
//! gpui drives the UI on its own executor. Work that needs tokio (reqwest, axum, sqlx) is
//! spawned here, and a gpui task awaits its `JoinHandle`, which works from any executor.

use std::future::Future;
use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("garden-io")
            .enable_all()
            .build()
            .expect("a tokio runtime")
    })
}

/// Runs `future` on the runtime. Await the result from a gpui task:
/// `cx.spawn(async move |this, cx| { let r = runtime::run(fut).await; ... })`.
pub async fn run<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> T {
    match runtime().spawn(future).await {
        Ok(value) => value,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => panic!("background task cancelled: {e}"),
    }
}
