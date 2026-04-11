//! Identity resolution for `castors add`.
//!
//! "Identity" here means the two pieces of information that uniquely
//! describe a castor in the registry: its [`CastorName`] and the
//! [`ImageTag`] its container is built from. Both can come from several
//! places, with a fixed precedence chain:
//!
//! - **Image**: CLI flag > `castor.image` (project) > `defaults.image` (global)
//!   → otherwise [`IdentityError::NoImage`].
//! - **Name**: CLI flag > `castor.name` (project) → otherwise auto-generated
//!   `<sanitized-dir-basename>-<n>` where `n` is the smallest positive
//!   integer that produces a name not already in the registry.
//!
//! The resolver is a pure function: it takes an `is_taken` closure rather
//! than a `&Registry`, so it can be unit-tested without any I/O.

use std::path::Path;
use std::str::FromStr;

use thiserror::Error;

use super::schema::{GlobalConfig, ProjectConfig};
use crate::core::domain::{CastorName, ImageTag};

/// Errors produced by identity resolution.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error(
        "no image specified: pass `-i IMAGE`, set `castor.image` in the project config, or set `defaults.image` in the global config"
    )]
    NoImage,
    #[error(
        "explicit name '{0}' is already taken; pick another with `-n NAME` or `castors rm {0}` first"
    )]
    NameTaken(CastorName),
}

/// Resolves the `(name, image)` pair for a castor about to be created.
///
/// The `is_taken` closure is consulted to detect collisions (for the explicit
/// case) and to find the next free auto-incremented suffix (for the auto
/// case). Pass `|n| registry.get(n).is_some()` from the call site.
///
/// # Errors
/// - [`IdentityError::NoImage`] if no layer provides an image.
/// - [`IdentityError::NameTaken`] if the explicit CLI/project name collides.
pub fn resolve_identity(
    cli_image: Option<ImageTag>,
    cli_name: Option<CastorName>,
    project: &ProjectConfig,
    global: &GlobalConfig,
    mount_dir: &Path,
    is_taken: impl Fn(&CastorName) -> bool,
) -> Result<(CastorName, ImageTag), IdentityError> {
    let image = cli_image
        .or_else(|| project.castor.image.clone())
        .or_else(|| global.defaults.image.clone())
        .ok_or(IdentityError::NoImage)?;

    let name = match cli_name.or_else(|| project.castor.name.clone()) {
        Some(explicit) => {
            if is_taken(&explicit) {
                return Err(IdentityError::NameTaken(explicit));
            }
            explicit
        }
        None => auto_name(&auto_name_base(mount_dir), is_taken),
    };

    Ok((name, image))
}

/// Returns a sanitized base name derived from the mount directory.
///
/// Rules: lowercase, replace any character that's not `[a-z0-9_]` with `-`,
/// collapse runs of `-`, trim leading/trailing `-`. If nothing usable
/// remains (e.g. the dir is `/`), falls back to the literal `castor`.
#[must_use]
pub fn auto_name_base(mount_dir: &Path) -> CastorName {
    let raw = mount_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");

    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_dash && !out.is_empty() {
                out.push('-');
                last_was_dash = true;
            }
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }

    let candidate = if out.is_empty() {
        "castor"
    } else {
        out.as_str()
    };

    // Sanitization above is engineered to always produce a valid CastorName
    // (ASCII alphanumeric / `-` / `_`). The fallback "castor" is also valid.
    // Unwrap is safe; the `expect` documents the invariant.
    CastorName::from_str(candidate).expect("sanitization must yield a valid castor name")
}

/// Returns the smallest `<base>-<n>` (`n` >= 1) that `is_taken` reports as
/// free. This always terminates: in the worst case the registry has finite
/// size and `n` keeps growing.
fn auto_name(base: &CastorName, is_taken: impl Fn(&CastorName) -> bool) -> CastorName {
    for n in 1u64.. {
        let candidate = CastorName::from_str(&format!("{base}-{n}"))
            .expect("base + integer suffix is always a valid castor name");
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 range is exhausted before a free name is found");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn cn(s: &str) -> CastorName {
        CastorName::from_str(s).unwrap()
    }
    fn img(s: &str) -> ImageTag {
        ImageTag::from_str(s).unwrap()
    }
    fn never_taken(_: &CastorName) -> bool {
        false
    }

    // ----- auto_name_base ----------------------------------------------------

    #[test]
    fn auto_name_base_simple_dir() {
        assert_eq!(auto_name_base(Path::new("/work/myrepo")).as_str(), "myrepo");
    }

    #[test]
    fn auto_name_base_lowercases_and_replaces_specials() {
        assert_eq!(
            auto_name_base(Path::new("/work/My Repo!")).as_str(),
            "my-repo"
        );
    }

    #[test]
    fn auto_name_base_collapses_runs_and_trims() {
        assert_eq!(
            auto_name_base(Path::new("/work/--Foo  Bar--")).as_str(),
            "foo-bar"
        );
    }

    #[test]
    fn auto_name_base_falls_back_when_dir_unusable() {
        assert_eq!(auto_name_base(Path::new("/")).as_str(), "castor");
        assert_eq!(auto_name_base(Path::new("///")).as_str(), "castor");
    }

    // ----- resolve_identity --------------------------------------------------

    fn project_with_image(image: &str) -> ProjectConfig {
        ProjectConfig {
            castor: super::super::schema::ProjectCastor {
                image: Some(img(image)),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn global_with_image(image: &str) -> GlobalConfig {
        GlobalConfig {
            defaults: super::super::schema::GlobalDefaults {
                image: Some(img(image)),
            },
            ..Default::default()
        }
    }

    #[test]
    fn cli_image_beats_project_and_global() {
        let global = global_with_image("global:tag");
        let project = project_with_image("project:tag");

        let (_name, image) = resolve_identity(
            Some(img("cli:tag")),
            None,
            &project,
            &global,
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(image.as_str(), "cli:tag");
    }

    #[test]
    fn project_image_beats_global() {
        let global = global_with_image("global:tag");
        let project = project_with_image("project:tag");

        let (_name, image) = resolve_identity(
            None,
            None,
            &project,
            &global,
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(image.as_str(), "project:tag");
    }

    #[test]
    fn global_image_used_as_last_resort() {
        let global = global_with_image("global:tag");
        let project = ProjectConfig::default();

        let (_name, image) = resolve_identity(
            None,
            None,
            &project,
            &global,
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(image.as_str(), "global:tag");
    }

    #[test]
    fn no_image_anywhere_is_an_error() {
        let err = resolve_identity(
            None,
            None,
            &ProjectConfig::default(),
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap_err();

        assert_eq!(err, IdentityError::NoImage);
    }

    #[test]
    fn cli_name_takes_precedence_over_project() {
        let project = ProjectConfig {
            castor: super::super::schema::ProjectCastor {
                name: Some(cn("from-project")),
                image: Some(img("img:tag")),
            },
            ..Default::default()
        };

        let (name, _image) = resolve_identity(
            None,
            Some(cn("from-cli")),
            &project,
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(name.as_str(), "from-cli");
    }

    #[test]
    fn explicit_name_collision_is_an_error() {
        let taken: HashSet<CastorName> = [cn("from-cli")].into_iter().collect();

        let err = resolve_identity(
            Some(img("img:tag")),
            Some(cn("from-cli")),
            &ProjectConfig::default(),
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            |n| taken.contains(n),
        )
        .unwrap_err();

        assert_eq!(err, IdentityError::NameTaken(cn("from-cli")));
    }

    #[test]
    fn auto_name_starts_at_one_when_nothing_is_taken() {
        let (name, _image) = resolve_identity(
            Some(img("img:tag")),
            None,
            &ProjectConfig::default(),
            &GlobalConfig::default(),
            &PathBuf::from("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(name.as_str(), "myrepo-1");
    }

    #[test]
    fn auto_name_skips_taken_suffixes() {
        let taken: HashSet<CastorName> = [cn("myrepo-1"), cn("myrepo-2"), cn("myrepo-4")]
            .into_iter()
            .collect();

        let (name, _image) = resolve_identity(
            Some(img("img:tag")),
            None,
            &ProjectConfig::default(),
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            |n| taken.contains(n),
        )
        .unwrap();

        // Smallest free suffix is 3, not 5.
        assert_eq!(name.as_str(), "myrepo-3");
    }

    #[test]
    fn auto_name_uses_sanitized_basename() {
        let (name, _image) = resolve_identity(
            Some(img("img:tag")),
            None,
            &ProjectConfig::default(),
            &GlobalConfig::default(),
            Path::new("/work/My Repo!"),
            never_taken,
        )
        .unwrap();

        assert_eq!(name.as_str(), "my-repo-1");
    }

    #[test]
    fn project_name_used_when_cli_omits_it() {
        let project = ProjectConfig {
            castor: super::super::schema::ProjectCastor {
                name: Some(cn("preferred")),
                image: Some(img("img:tag")),
            },
            ..Default::default()
        };

        let (name, _image) = resolve_identity(
            None,
            None,
            &project,
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            never_taken,
        )
        .unwrap();

        assert_eq!(name.as_str(), "preferred");
    }

    #[test]
    fn project_name_collision_is_an_error_not_an_increment() {
        // Project name is treated as literal: if it collides, we error rather
        // than silently appending `-1`. Auto-increment only fires when nobody
        // chose a name.
        let project = ProjectConfig {
            castor: super::super::schema::ProjectCastor {
                name: Some(cn("preferred")),
                image: Some(img("img:tag")),
            },
            ..Default::default()
        };
        let taken: HashSet<CastorName> = [cn("preferred")].into_iter().collect();

        let err = resolve_identity(
            None,
            None,
            &project,
            &GlobalConfig::default(),
            Path::new("/work/myrepo"),
            |n| taken.contains(n),
        )
        .unwrap_err();

        assert_eq!(err, IdentityError::NameTaken(cn("preferred")));
    }
}
