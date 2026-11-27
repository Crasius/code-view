//! Names a checkout gives what it makes: the worktree's folder from the user's pattern and the
//! local branch of a pull request from a fork.

// RS-DONT-04: the input is a branch name from the host and a pattern the user typed.
#![warn(clippy::string_slice, clippy::indexing_slicing)]

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// The longest branch slug in a folder or branch name; long enough to tell branches apart.
const SLUG_MAX: usize = 40;

/// The last part of `branch`, fit for a folder or branch name: `feature/Login Cache` →
/// `login-cache`. Never empty.
pub(crate) fn slug(branch: &str) -> String {
    let last = branch.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    let mut slug = String::with_capacity(last.len());
    for character in last.chars().flat_map(char::to_lowercase) {
        let kept = if character.is_ascii_alphanumeric() || matches!(character, '.' | '_') {
            character
        } else {
            '-'
        };
        if !(kept == '-' && slug.ends_with('-')) {
            slug.push(kept);
        }
    }
    let trimmed: String = slug.trim_matches(['-', '.']).chars().take(SLUG_MAX).collect();
    let trimmed = trimmed.trim_end_matches(['-', '.']);
    if trimmed.is_empty() { "branch".to_owned() } else { trimmed.to_owned() }
}

/// The local branch of a pull request whose branch lives in a fork: `pr-12-login-cache`. No
/// remote of the clone has that branch, so the name says where it came from.
pub fn fork_branch(number: u64, branch: &str) -> String {
    format!("pr-{number}-{}", slug(branch))
}

/// The folder a new worktree of pull request `number` gets: `pattern` with `{clone}` (the clone's
/// folder), `{number}` and `{branch}` (its `slug`) filled in. `/` and `\` both separate folders.
/// A relative result is taken from the clone's parent folder.
pub fn worktree_path(pattern: &str, clone: &Path, number: u64, branch: &str) -> PathBuf {
    let pattern = pattern.trim();
    let clone_text = clone.to_string_lossy();
    let clone_text = clone_text.trim_end_matches(['/', '\\']);
    let expanded = pattern
        .replace("{clone}", clone_text)
        .replace("{number}", &number.to_string())
        .replace("{branch}", &slug(branch));
    let native: String = expanded
        .chars()
        .map(|character| if matches!(character, '/' | '\\') { MAIN_SEPARATOR } else { character })
        .collect();
    let path = PathBuf::from(native);
    if path.is_absolute() || path.has_root() {
        return path;
    }
    clone.parent().map_or_else(|| path.clone(), |parent| parent.join(&path))
}

#[cfg(test)]
mod tests {
    use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

    use super::{slug, worktree_path};

    #[test]
    fn slugs_keep_the_last_part_of_the_branch_in_lower_case() {
        assert_eq!(slug("feature/login-cache"), "login-cache");
        assert_eq!(slug("Feature/Login Cache!!"), "login-cache");
        assert_eq!(slug("fix_v2.1"), "fix_v2.1");
        assert_eq!(slug("users/ana/"), "ana");
        assert_eq!(slug("---"), "branch", "never empty");
        assert_eq!(slug("ümlaut-branch"), "mlaut-branch");
        assert_eq!(slug(&"x".repeat(80)).len(), 40);
    }

    fn native(path: &str) -> PathBuf {
        PathBuf::from(path.replace('/', MAIN_SEPARATOR_STR))
    }

    #[test]
    fn worktree_folders_follow_the_pattern_beside_the_clone() {
        let clone = native("/dev/login-app");
        let pattern = "{clone}-wt\\pr-{number}-{branch}";
        assert_eq!(
            worktree_path(pattern, &clone, 12, "feature/login-cache"),
            native("/dev/login-app-wt/pr-12-login-cache")
        );
        assert_eq!(
            worktree_path("reviews/{number}", &clone, 3, "main"),
            native("/dev/reviews/3"),
            "a relative pattern starts beside the clone"
        );
        assert_eq!(
            worktree_path("{clone}/../wt-{number}", Path::new(&native("/dev/app/")), 9, "x"),
            native("/dev/app/../wt-9"),
            "a trailing separator of the clone does not double"
        );
    }
}
