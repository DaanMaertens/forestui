use super::*;

fn naming(tools: &[&str]) -> SessionNaming {
    SessionNaming {
        enabled: true,
        model: "haiku".into(),
        timeout_secs: 20,
        allowed_tools: tools.iter().map(|t| t.to_string()).collect(),
    }
}

/// With no tools allowed, the call gets none at all, and nothing is
/// left behind: no transcript and no hooks.
#[test]
fn by_default_the_call_has_no_tools_and_leaves_nothing_behind() {
    let args = arguments(&naming(&[]));
    let joined = args.join(" ");
    assert!(joined.starts_with("-p --model haiku"), "{joined}");
    assert!(args.contains(&"--no-session-persistence".to_string()));
    assert!(
        joined.contains(r#"--settings {"disableAllHooks":true}"#),
        "{joined}"
    );
    let tools = args.iter().position(|a| a == "--tools").expect("--tools");
    assert_eq!(args[tools + 1], "");
    assert!(!args.contains(&"--allowedTools".to_string()), "{joined}");
}

/// The allowed tools are offered and allowed outright, joined into one
/// argument per flag, and nothing follows them that could be read as
/// another tool.
#[test]
fn allowed_tools_are_offered_and_allowed() {
    let args = arguments(&naming(&["Read", "Grep"]));
    let tools = args.iter().position(|a| a == "--tools").expect("--tools");
    assert_eq!(args[tools + 1], "Read,Grep");
    let allowed = args
        .iter()
        .position(|a| a == "--allowedTools")
        .expect("allowed");
    assert_eq!(args[allowed + 1], "Read,Grep");
    assert_eq!(
        args.len(),
        allowed + 2,
        "nothing follows the tools: {args:?}"
    );
}
