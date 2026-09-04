use std::sync::Arc;

pub fn install(handler: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<(), String> {
    crate::platform::install_termination_handler(handler)
}

pub fn reply_to_termination_request() {
    crate::platform::reply_to_termination_request();
}
