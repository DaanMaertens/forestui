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
