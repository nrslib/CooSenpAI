use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;

type Barrier = (Arc<Notify>, Arc<Notify>);

fn barriers() -> &'static Mutex<HashMap<String, Barrier>> {
    static BARRIERS: OnceLock<Mutex<HashMap<String, Barrier>>> = OnceLock::new();
    BARRIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn arm(key: &str) -> (Arc<Notify>, Arc<Notify>) {
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    barriers()
        .lock()
        .expect("runtime test barrier lock")
        .insert(key.to_owned(), (reached.clone(), release.clone()));
    (reached, release)
}

pub(crate) async fn wait(key: &str) {
    let barrier = barriers()
        .lock()
        .expect("runtime test barrier lock")
        .remove(key);
    if let Some((reached, release)) = barrier {
        reached.notify_one();
        release.notified().await;
    }
}
