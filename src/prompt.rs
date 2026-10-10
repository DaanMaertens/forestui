//! Prompt templates: the text that, with a work item filled in, becomes
//! Claude's first prompt.

use std::path::Path;

/// The built-in `issue` template, used until templates become editable files.
pub const ISSUE_TEMPLATE: &str = include_str!("../assets/prompts/issue.md");

/// The built-in `feature` template: plan first, build once the plan is approved.
pub const FEATURE_TEMPLATE: &str = include_str!("../assets/prompts/feature.md");

/// The built-in `free-feature` template: the typed description as `{body}`,
/// and the same plan-first rule as a feature from an issue.
pub const FREE_FEATURE_TEMPLATE: &str = include_str!("../assets/prompts/free-feature.md");

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
mod tests;
