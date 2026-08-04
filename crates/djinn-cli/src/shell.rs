/// Quote a value for copy-pasteable shell command hints.
///
/// This deliberately always quotes so command hints stay stable and paths with
/// spaces or metacharacters are safe to paste.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Quote only when a value is not a simple shell word.
///
/// Use this for commands that are easier to read without unconditional quotes,
/// but still need safe output for whitespace or shell metacharacters.
pub(crate) fn shell_quote_if_needed(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '='))
    {
        value.to_string()
    } else {
        shell_quote(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_always_wraps_values_for_stable_command_hints() {
        assert_eq!(shell_quote("simple"), "'simple'");
        assert_eq!(shell_quote("path with spaces"), "'path with spaces'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_if_needed_leaves_safe_words_readable() {
        assert_eq!(
            shell_quote_if_needed("tools/buddy/bin/buddy"),
            "tools/buddy/bin/buddy"
        );
        assert_eq!(
            shell_quote_if_needed("path with spaces"),
            "'path with spaces'"
        );
    }
}
