use std::path::PathBuf;

pub(crate) fn expand_tilde_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return djinn_core::home_dir().join(rest);
    }
    PathBuf::from(value)
}
