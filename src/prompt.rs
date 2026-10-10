//! Prompt templates: the text that, with a work item filled in, becomes
//! Claude's first prompt.

use std::path::Path;

/// The built-in `issue` template, used until templates become editable files.
pub const ISSUE_TEMPLATE: &str = include_str!("../assets/prompts/issue.md");

/// The built-in `feature` template: plan first, build once the plan is approved.
pub const FEATURE_TEMPLATE: &str = include_str!("../assets/prompts/feature.md");

/// What a template's placeholders are filled with.
#[derive(Debug, Clone, Default)]
pub struct TemplateVars<'a> {
    pub number: Option<i64>,
    pub title: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub repo: &'a str,
    /// The typed description of a free-text feature, and nothing else: an
    /// issue's or PR's body would be a stale copy of text Claude can read
    /// fresh with `gh`.
    pub body: Option<&'a str>,
}

/// `template` with every known placeholder replaced.
///
/// One pass, left to right, so a value is never itself expanded — an issue
/// titled `{branch}` stays that title. A placeholder this does not know, or
/// `{body}` outside a free-text feature, is left as written rather than
/// failing: a typo in a template should show up in the prompt, not stop the
/// launch.
pub fn fill(template: &str, vars: &TemplateVars<'_>) -> String {
    let number = vars.number.map(|n| n.to_string());
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let value = match &after[..close] {
                "number" => number.as_deref(),
                "title" => Some(vars.title),
                "branch" => Some(vars.branch),
                "worktree" => Some(vars.worktree),
                "repo" => Some(vars.repo),
                "body" => vars.body,
                _ => None,
            };
            value.map(|v| (v, close))
        });
        match value {
            Some((value, close)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Let `edit` change `text` through a file, and return what it left there.
///
/// The file lives in the system temp dir, never in the worktree, so an edit
/// cannot show up as a change there or be committed by Claude; and it is
/// removed whatever `edit` reports. The text stays with the caller: nothing
/// here writes back to a template, so an edit applies to one launch only.
pub fn edit_via_file(
    text: &str,
    edit: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!("forestui-prompt-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let edited = edit(&path).and_then(|()| {
        std::fs::read_to_string(&path).map_err(|e| format!("could not read the prompt back: {e}"))
    });
    let _ = std::fs::remove_file(&path);
    edited
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue_vars() -> TemplateVars<'static> {
        TemplateVars {
            number: Some(42),
            title: "Fix login bug",
            branch: "feat/42-fix-login-bug",
            worktree: "42-fix-login-bug",
            repo: "forestui",
            body: None,
        }
    }

    #[test]
    fn every_known_placeholder_is_filled() {
        assert_eq!(
            fill(
                "#{number} {title} on {branch} in {worktree} of {repo}",
                &issue_vars()
            ),
            "#42 Fix login bug on feat/42-fix-login-bug in 42-fix-login-bug of forestui"
        );
    }

    #[test]
    fn unknown_placeholders_and_stray_braces_are_left_as_written() {
        assert_eq!(
            fill("{nmber} {} { title} {title", &issue_vars()),
            "{nmber} {} { title} {title"
        );
    }

    #[test]
    fn body_is_filled_for_free_text_only() {
        assert_eq!(fill("do: {body}", &issue_vars()), "do: {body}");
        let free = TemplateVars {
            number: None,
            body: Some("dark mode"),
            ..issue_vars()
        };
        assert_eq!(fill("do: {body}", &free), "do: dark mode");
        // A free-text feature has no number to fill in.
        assert_eq!(fill("#{number}", &free), "#{number}");
    }

    #[test]
    fn a_value_is_never_expanded_again() {
        let vars = TemplateVars {
            title: "use {branch} here",
            ..issue_vars()
        };
        assert_eq!(fill("{title}", &vars), "use {branch} here");
    }

    #[test]
    fn the_issue_template_names_the_issue_and_reads_it_fresh() {
        let prompt = fill(ISSUE_TEMPLATE, &issue_vars());
        assert!(prompt.contains("#42"), "{prompt}");
        assert!(prompt.contains("Fix login bug"), "{prompt}");
        assert!(prompt.contains("gh issue view 42 --comments"), "{prompt}");
        assert!(
            prompt.contains("propose a fix before changing any code"),
            "{prompt}"
        );
        assert!(!prompt.contains('{'), "an unfilled placeholder: {prompt}");
    }

    #[test]
    fn the_feature_template_plans_and_waits_for_approval() {
        let prompt = fill(FEATURE_TEMPLATE, &issue_vars());
        assert!(prompt.contains("#42: Fix login bug"), "{prompt}");
        assert!(prompt.contains("gh issue view 42 --comments"), "{prompt}");
        assert!(prompt.contains("write a plan"), "{prompt}");
        assert!(prompt.contains("approved the plan"), "{prompt}");
        assert!(!prompt.contains('{'), "an unfilled placeholder: {prompt}");
    }

    /// The editor sees the prompt in a file outside any worktree, what it
    /// saves comes back, and the file is gone afterwards.
    #[test]
    fn an_edit_round_trips_through_a_temp_file() {
        let mut seen = None;
        let edited = edit_via_file("Work on issue 3", |path| {
            seen = Some(path.to_path_buf());
            assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
            assert_eq!(std::fs::read_to_string(path).unwrap(), "Work on issue 3");
            std::fs::write(path, "Work on issue 3, carefully\n").map_err(|e| e.to_string())
        });
        assert_eq!(edited.as_deref(), Ok("Work on issue 3, carefully\n"));
        let seen = seen.expect("the editor ran");
        assert!(!seen.exists(), "{} left behind", seen.display());
    }

    #[test]
    fn a_failed_edit_still_removes_the_file() {
        let mut seen = None;
        let edited = edit_via_file("Work on issue 3", |path| {
            seen = Some(path.to_path_buf());
            Err("no editor".to_string())
        });
        assert_eq!(edited, Err("no editor".to_string()));
        assert!(!seen.expect("the editor ran").exists());
    }
}
