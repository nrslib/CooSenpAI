use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;

type Barrier = (Arc<Notify>, Arc<Notify>);

fn barriers() -> &'static Mutex<HashMap<String, Barrier>> {
    static BARRIERS: OnceLock<Mutex<HashMap<String, Barrier>>> = OnceLock::new();
    BARRIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn arm(message: &str) -> (Arc<Notify>, Arc<Notify>) {
    let barrier = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    barriers()
        .lock()
        .expect("user response barriers")
        .insert(message.to_owned(), barrier.clone());
    barrier
}

pub(crate) async fn wait_with_cancellation(
    message: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) {
    let barrier = barriers()
        .lock()
        .expect("user response barriers")
        .remove(message);
    if let Some((reached, release)) = barrier {
        reached.notify_one();
        tokio::select! {
            _ = release.notified() => {}
            _ = cancellation.cancelled() => {}
        }
    }
}
