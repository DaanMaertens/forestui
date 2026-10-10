//! Prompt templates: the text that, with a work item filled in, becomes
//! Claude's first prompt.

/// The built-in `issue` template, used until templates become editable files.
pub const ISSUE_TEMPLATE: &str = include_str!("../assets/prompts/issue.md");

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
        assert!(!prompt.contains('{'), "an unfilled placeholder: {prompt}");
    }
}
