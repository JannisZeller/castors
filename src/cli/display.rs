//! Small presentation helpers shared by CLI commands.

use std::path::Path;

/// Formats an absolute path for display, abbreviating the user's home
/// directory prefix with `~` when applicable. Returns the plain display form
/// otherwise.
#[must_use]
pub fn format_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_owned();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}
