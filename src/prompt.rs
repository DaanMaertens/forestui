//! Prompt templates: the text that, with a work item filled in, becomes
//! Claude's first prompt.

use std::path::{Path, PathBuf};

/// The built-in `issue` template: investigate and propose before fixing.
pub const ISSUE_TEMPLATE: &str = include_str!("../assets/prompts/issue.md");

/// The built-in `feature` template: plan first, build once the plan is approved.
pub const FEATURE_TEMPLATE: &str = include_str!("../assets/prompts/feature.md");

/// The built-in `free-feature` template: the typed description as `{body}`,
/// and the same plan-first rule as a feature from an issue.
pub const FREE_FEATURE_TEMPLATE: &str = include_str!("../assets/prompts/free-feature.md");

/// The built-in `pr-review-others` template: review someone else's PR and
/// report, changing nothing.
pub const PR_REVIEW_OTHERS_TEMPLATE: &str = include_str!("../assets/prompts/pr-review-others.md");

/// The built-in `pr-review-own` template: self-review my own PR, list the
/// findings, and fix only after a go-ahead.
pub const PR_REVIEW_OWN_TEMPLATE: &str = include_str!("../assets/prompts/pr-review-own.md");

/// One prompt template. Each is a Markdown file of the same name in a
/// `prompts` directory, which overrides the default embedded here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Issue,
    Feature,
    FreeFeature,
    PrReviewOthers,
    PrReviewOwn,
}

impl Template {
    /// Every template, in the order the Settings picker lists them.
    pub const ALL: [Template; 5] = [
        Template::Issue,
        Template::Feature,
        Template::FreeFeature,
        Template::PrReviewOthers,
        Template::PrReviewOwn,
    ];

    /// The file that overrides this template's default.
    pub fn file_name(self) -> &'static str {
        match self {
            Template::Issue => "issue.md",
            Template::Feature => "feature.md",
            Template::FreeFeature => "free-feature.md",
            Template::PrReviewOthers => "pr-review-others.md",
            Template::PrReviewOwn => "pr-review-own.md",
        }
    }

    /// What the template is for, as the Settings picker lists it.
    pub fn label(self) -> &'static str {
        match self {
            Template::Issue => "Issue",
            Template::Feature => "Feature",
            Template::FreeFeature => "Free-text feature",
            Template::PrReviewOthers => "Review someone else's PR",
            Template::PrReviewOwn => "Review my own PR",
        }
    }

    pub fn default_text(self) -> &'static str {
        match self {
            Template::Issue => ISSUE_TEMPLATE,
            Template::Feature => FEATURE_TEMPLATE,
            Template::FreeFeature => FREE_FEATURE_TEMPLATE,
            Template::PrReviewOthers => PR_REVIEW_OTHERS_TEMPLATE,
            Template::PrReviewOwn => PR_REVIEW_OWN_TEMPLATE,
        }
    }
}

/// A template's text, and why it is not the file's if the file was there
/// but could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub text: String,
    pub warning: Option<String>,
}

/// `template` from the first of `layers` with a usable file for it — a
/// repository's prompts, then the global ones — or its built-in default.
///
/// A missing file is the normal case and says nothing. A file that is there
/// but unusable — unreadable, not UTF-8, or blank — falls through to the
/// next layer too, since a broken template must not stop a launch, but with
/// a warning: the user wrote that file and expects it to be used.
pub fn resolve(layers: &[&Path], template: Template) -> Resolved {
    let mut problems = Vec::new();
    for dir in layers {
        let path = dir.join(template.file_name());
        let why = match std::fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => "is empty".to_string(),
            Ok(text) => {
                return Resolved {
                    warning: warning(&problems, &path.display().to_string()),
                    text,
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => format!("could not be read ({e})"),
        };
        problems.push(format!("{} {why}", path.display()));
    }
    Resolved {
        text: template.default_text().to_string(),
        warning: warning(&problems, "the built-in default"),
    }
}

fn warning(problems: &[String], used: &str) -> Option<String> {
    (!problems.is_empty()).then(|| format!("Prompt template {}; using {used}", problems.join(", ")))
}

/// The path of `template`'s file in `dir`, written first if it is not there
/// yet with the text `fallbacks` give it — the global template for a
/// repository's, the built-in default for a global one — so the editor
/// opens on the text in use rather than on an empty file. A file that
/// exists is never touched.
pub fn ensure_file(
    dir: &Path,
    template: Template,
    fallbacks: &[&Path],
) -> std::io::Result<PathBuf> {
    let path = dir.join(template.file_name());
    if !path.exists() {
        std::fs::create_dir_all(dir)?;
        crate::util::write_atomically(&path, &resolve(fallbacks, template).text)?;
    }
    Ok(path)
}

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
