use coosenpai_core::config::ConfigError;

pub(crate) fn format(error: &anyhow::Error) -> String {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ConfigError>())
        .map_or_else(|| format!("{error:#}"), ConfigError::format_for_user)
}
