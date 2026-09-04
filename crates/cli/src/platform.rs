pub use coosenpai_platform_macos::*;

pub fn provider_api_key_store() -> std::sync::Arc<dyn coosenpai_core::ports::ProviderApiKeyStore> {
    std::sync::Arc::new(coosenpai_platform_macos::MacKeychain)
}
