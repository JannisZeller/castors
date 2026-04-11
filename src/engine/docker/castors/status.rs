//! Parsing of `docker inspect` / `docker ps` output into engine types.
//!
//! Pure string-in / type-out functions, kept separate from the subprocess
//! plumbing so they can be tested without docker.

use crate::core::domain::CastorName;
use crate::engine::types::{CastorStatus, ManagedContainer};

/// Parses the output of `docker inspect --format '{{.State.Status}}|{{.State.ExitCode}}'`.
///
/// Anything other than `running` is treated as an exited state. The exit code
/// is recorded when present.
#[must_use]
pub fn parse_inspect_status(stdout: &str) -> CastorStatus {
    let line = stdout.trim();
    let (status, exit) = line.split_once('|').unwrap_or((line, ""));
    if status == "running" {
        CastorStatus::Running
    } else {
        CastorStatus::Exited {
            exit_code: exit.trim().parse().ok(),
        }
    }
}

/// Parses one line of `docker ps --format '{{.Label "castors.name"}}|{{.State}}'`.
///
/// Returns `None` for lines that do not contain a valid castor name (e.g.
/// containers missing the label, which would not normally appear given the
/// filter, but we defend against it).
#[must_use]
pub fn parse_list_line(line: &str) -> Option<ManagedContainer> {
    let (name, state) = line.split_once('|')?;
    let name = name.trim().parse::<CastorName>().ok()?;
    let status = match state.trim() {
        "running" => CastorStatus::Running,
        // `docker ps` doesn't include exit codes; drill in via inspect_status if needed.
        _ => CastorStatus::Exited { exit_code: None },
    };
    Some(ManagedContainer { name, status })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_status() {
        assert_eq!(parse_inspect_status("running|0\n"), CastorStatus::Running);
    }

    #[test]
    fn parses_exited_with_exit_code() {
        assert_eq!(
            parse_inspect_status("exited|137\n"),
            CastorStatus::Exited {
                exit_code: Some(137)
            }
        );
    }

    #[test]
    fn parses_exited_without_exit_code() {
        assert_eq!(
            parse_inspect_status("created|"),
            CastorStatus::Exited { exit_code: None }
        );
    }

    #[test]
    fn parses_list_line_running() {
        let mc = parse_list_line("alpha|running").unwrap();
        assert_eq!(mc.name.as_str(), "alpha");
        assert_eq!(mc.status, CastorStatus::Running);
    }

    #[test]
    fn parses_list_line_exited() {
        let mc = parse_list_line("beta|exited").unwrap();
        assert_eq!(mc.name.as_str(), "beta");
        assert_eq!(mc.status, CastorStatus::Exited { exit_code: None });
    }

    #[test]
    fn list_line_without_pipe_is_skipped() {
        assert!(parse_list_line("just-a-name").is_none());
    }

    #[test]
    fn list_line_with_invalid_name_is_skipped() {
        assert!(parse_list_line("bad name!|running").is_none());
    }
}
