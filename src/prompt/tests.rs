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
fn body_is_filled_for_a_session_only() {
    assert_eq!(fill("do: {body}", &issue_vars()), "do: {body}");
    let session = TemplateVars {
        number: None,
        body: Some("dark mode"),
        ..issue_vars()
    };
    assert_eq!(fill("do: {body}", &session), "do: dark mode");
    // A session prompt has no number to fill in.
    assert_eq!(fill("#{number}", &session), "#{number}");
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

fn pr_vars() -> TemplateVars<'static> {
    TemplateVars {
        number: Some(31),
        title: "Give the cache tests their own cache",
        branch: "fix/github-cache-test-race",
        worktree: "pr-31-give-the-cache-tests-their-own-cache",
        repo: "forestui",
        body: None,
    }
}

/// Someone else's PR is read fresh, reviewed and reported on — no edits.
#[test]
fn the_others_review_template_reports_without_changing_code() {
    let prompt = fill(PR_REVIEW_OTHERS_TEMPLATE, &pr_vars());
    assert!(prompt.contains("#31: Give the cache tests"), "{prompt}");
    assert!(prompt.contains("gh pr view 31 --comments"), "{prompt}");
    assert!(prompt.contains("`fix/github-cache-test-race`"), "{prompt}");
    assert!(prompt.contains("Do not change any code"), "{prompt}");
    assert!(!prompt.contains('{'), "an unfilled placeholder: {prompt}");
}

/// My own PR is self-reviewed: findings first, fixes only after a go-ahead.
#[test]
fn the_own_review_template_waits_before_fixing() {
    let prompt = fill(PR_REVIEW_OWN_TEMPLATE, &pr_vars());
    assert!(prompt.contains("my own pull request #31"), "{prompt}");
    assert!(prompt.contains("gh pr view 31 --comments"), "{prompt}");
    assert!(prompt.contains("list your findings"), "{prompt}");
    assert!(prompt.contains("wait for my go-ahead"), "{prompt}");
    assert!(!prompt.contains('{'), "an unfilled placeholder: {prompt}");
}

/// The session template is the session prompt, verbatim: no plan-first
/// rule, no issue to read.
#[test]
fn the_session_template_is_the_prompt_alone() {
    let session = TemplateVars {
        number: None,
        title: "Why is the sidebar slow?",
        body: Some("Why is the sidebar slow?"),
        branch: "session/session-why-is-the-sidebar-slow",
        worktree: "session-why-is-the-sidebar-slow",
        repo: "forestui",
    };
    assert_eq!(
        fill(SESSION_TEMPLATE, &session),
        "Why is the sidebar slow?\n"
    );
}

/// An override of the session template can say where Claude is.
#[test]
fn a_session_override_fills_the_other_placeholders() {
    let session = TemplateVars {
        number: None,
        title: "Spike",
        body: Some("Spike"),
        branch: "session/session-spike",
        worktree: "session-spike",
        repo: "forestui",
    };
    assert_eq!(
        fill("{body} ({worktree} of {repo}, on {branch})", &session),
        "Spike (session-spike of forestui, on session/session-spike)"
    );
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

#[test]
fn without_a_file_every_template_is_its_built_in_default() {
    let dir = tempfile::tempdir().unwrap();
    for template in Template::ALL {
        let resolved = resolve(&[dir.path()], template);
        assert_eq!(resolved.text, template.default_text(), "{template:?}");
        assert_eq!(resolved.warning, None, "{template:?}");
    }
}

#[test]
fn a_template_file_is_used_instead_of_the_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("issue.md"), "Fix #{number} my way").unwrap();

    let resolved = resolve(&[dir.path()], Template::Issue);
    assert_eq!(resolved.text, "Fix #{number} my way");
    assert_eq!(resolved.warning, None);
    // Each template has its own file: the others keep their defaults.
    assert_eq!(
        resolve(&[dir.path()], Template::Feature).text,
        FEATURE_TEMPLATE
    );
}

#[test]
fn the_file_names_are_the_template_names() {
    let names: Vec<_> = Template::ALL.iter().map(|t| t.file_name()).collect();
    assert_eq!(
        names,
        [
            "issue.md",
            "feature.md",
            "session.md",
            "session-name.md",
            "pr-review-others.md",
            "pr-review-own.md"
        ]
    );
}

#[test]
fn an_unreadable_file_falls_back_to_the_default_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    // Not UTF-8, so it cannot be read as text.
    std::fs::write(dir.path().join("feature.md"), [0xff, 0xfe, 0x00]).unwrap();
    // A directory where the file should be cannot be read at all.
    std::fs::create_dir(dir.path().join("issue.md")).unwrap();

    for template in [Template::Feature, Template::Issue] {
        let resolved = resolve(&[dir.path()], template);
        assert_eq!(resolved.text, template.default_text(), "{template:?}");
        let warning = resolved.warning.expect("a warning");
        assert!(warning.contains(template.file_name()), "{warning}");
    }
}

#[test]
fn an_empty_file_falls_back_to_the_default_with_a_warning() {
    // An empty first prompt would start Claude on nothing; a file emptied by
    // accident should not do that silently.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pr-review-own.md"), " \n\n").unwrap();

    let resolved = resolve(&[dir.path()], Template::PrReviewOwn);
    assert_eq!(resolved.text, PR_REVIEW_OWN_TEMPLATE);
    assert!(resolved.warning.is_some());
}

#[test]
fn ensuring_a_file_creates_it_from_the_default_and_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = dir.path().join("prompts");

    let path = ensure_file(&prompts, Template::Session, &[]).unwrap();
    assert_eq!(path, prompts.join("session.md"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SESSION_TEMPLATE);
}

#[test]
fn ensuring_a_file_never_overwrites_one_that_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("issue.md");
    std::fs::write(&path, "mine").unwrap();

    assert_eq!(ensure_file(dir.path(), Template::Issue, &[]).unwrap(), path);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
}

#[test]
fn a_repository_template_overrides_the_global_one() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, global) = (dir.path().join("repo"), dir.path().join("global"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(repo.join("issue.md"), "repo issue").unwrap();
    std::fs::write(global.join("issue.md"), "global issue").unwrap();
    std::fs::write(global.join("feature.md"), "global feature").unwrap();
    let layers = [repo.as_path(), global.as_path()];

    assert_eq!(resolve(&layers, Template::Issue).text, "repo issue");
    // A template the repository leaves alone is the global one, then the
    // built-in default.
    assert_eq!(resolve(&layers, Template::Feature).text, "global feature");
    assert_eq!(
        resolve(&layers, Template::PrReviewOwn).text,
        PR_REVIEW_OWN_TEMPLATE
    );
}

#[test]
fn an_unusable_repository_template_falls_back_to_the_global_one_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, global) = (dir.path().join("repo"), dir.path().join("global"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(repo.join("issue.md"), "").unwrap();
    std::fs::write(global.join("issue.md"), "global issue").unwrap();

    let resolved = resolve(&[repo.as_path(), global.as_path()], Template::Issue);
    assert_eq!(resolved.text, "global issue");
    let warning = resolved.warning.expect("a warning");
    assert!(warning.contains("repo/issue.md"), "{warning}");
    assert!(
        warning.contains("global/issue.md"),
        "names the one used: {warning}"
    );
}

#[test]
fn ensuring_a_file_prefills_it_from_the_layers_below() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, global) = (dir.path().join("repo"), dir.path().join("global"));
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(global.join("issue.md"), "global issue").unwrap();

    let path = ensure_file(&repo, Template::Issue, &[global.as_path()]).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "global issue");
}

/// The name suggestion's instruction asks for a short kebab-case name and
/// nothing else, with the session prompt as its `{body}`.
#[test]
fn the_session_name_template_asks_for_a_kebab_case_name_for_the_prompt() {
    assert_eq!(Template::SessionName.file_name(), "session-name.md");
    assert!(SESSION_NAME_TEMPLATE.contains("kebab-case"));
    let filled = fill(
        SESSION_NAME_TEMPLATE,
        &TemplateVars {
            body: Some("Why is the sidebar slow?"),
            ..TemplateVars::default()
        },
    );
    assert!(filled.contains("Why is the sidebar slow?"), "{filled}");
    assert!(!filled.contains("{body}"), "{filled}");
}
