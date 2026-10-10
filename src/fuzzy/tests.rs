use super::*;

#[test]
fn strip_remote_prefix_uses_real_remotes() {
    let remotes = vec!["origin".to_string(), "upstream".to_string()];
    assert_eq!(strip_remote_prefix("origin/main", &remotes), "main");
    assert_eq!(strip_remote_prefix("feat/thing", &remotes), "feat/thing");
}

#[test]
fn fuzzy_scoring_tiers() {
    let remotes = vec!["origin".to_string()];
    let branches: Vec<String> = ["main", "origin/main", "feat/login", "release-2"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let out = fuzzy_match_branches("main", &branches, &remotes, 10);
    assert_eq!(out[0].0, "main");
    assert_eq!(out[0].1, 0.0);
    assert_eq!(out[1].0, "origin/main");
    assert_eq!(out[1].1, 0.5);

    let out = fuzzy_match_branches("login", &branches, &remotes, 10);
    assert_eq!(out[0].0, "feat/login");
    assert_eq!(out[0].1, 2.0);

    // Empty query returns everything, capped.
    let out = fuzzy_match_branches("", &branches, &remotes, 2);
    assert_eq!(out.len(), 2);

    // Typo still matches through the Levenshtein tier.
    let out = fuzzy_match_branches("relase", &branches, &remotes, 10);
    assert!(out.iter().any(|(b, _)| b == "release-2"));
}

/// One branch per scoring tier, in scrambled order, so the whole ranking
/// is pinned before the matcher stops being branch-specific. Every score
/// was worked out by hand from the tiers on `match_score`.
#[test]
fn ranks_branches_through_every_scoring_tier() {
    let remotes = vec!["origin".to_string()];
    let branches: Vec<String> = [
        "main",
        "defeat",
        "fxatures",
        "fest",
        "origin/feat/login",
        "B-feat",
        "feat/login",
        "origin/feat",
        "a-feat",
        "Feat",
        "origin/main",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let out = fuzzy_match_branches("feat", &branches, &remotes, 50);

    let ranked: Vec<(&str, f64)> = out.iter().map(|(b, s)| (b.as_str(), *s)).collect();
    assert_eq!(
        ranked,
        vec![
            ("Feat", 0.0),              // exact, ignoring case
            ("origin/feat", 0.5),       // exact on the local name
            ("feat/login", 1.0),        // prefix
            ("origin/feat/login", 1.5), // prefix on the local name
            ("a-feat", 2.0),            // substring after a separator...
            ("B-feat", 2.0),            // ...ties sort by name, ignoring case
            ("defeat", 3.0),            // substring anywhere
            ("fest", 4.1),              // one edit away from a whole segment
            ("fxatures", 4.6),          // one edit away from a segment's prefix
        ]
    );
}

/// The remote prefix is stripped from the *lowercased* branch and compared
/// with the remote names as spelled. So `Origin/feat` counts as a remote
/// branch of `origin`, while a remote spelled `Origin` never strips at
/// all. Pinned so that precomputing the local name keeps this quirk.
#[test]
fn remote_prefix_is_stripped_after_lowercasing() {
    let branches = vec!["Origin/feat".to_string()];

    let lower_remote = vec!["origin".to_string()];
    let out = fuzzy_match_branches("feat", &branches, &lower_remote, 50);
    assert_eq!(out, vec![("Origin/feat".to_string(), 0.5)]); // exact on local name

    let upper_remote = vec!["Origin".to_string()];
    let out = fuzzy_match_branches("feat", &branches, &upper_remote, 50);
    assert_eq!(out, vec![("Origin/feat".to_string(), 2.0)]); // substring after `/`
}

/// A blank query lists entries as given, not sorted by name, so a list
/// that arrives ordered (recent branches first) keeps that order.
#[test]
fn a_blank_query_keeps_the_given_order() {
    let branches: Vec<String> = ["zeta", "Alpha", "mid"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    for query in ["", "   "] {
        let out = fuzzy_match_branches(query, &branches, &[], 2);
        assert_eq!(
            out,
            vec![("zeta".to_string(), 0.0), ("Alpha".to_string(), 0.0)]
        );
    }
}

/// The smallest possible entry: one text and no short form.
struct Entry(&'static str);

impl Searchable for Entry {
    fn search_text(&self) -> &str {
        self.0
    }
}

#[test]
fn matches_any_searchable_list() {
    let items = [Entry("defeat"), Entry("feat/login"), Entry("main")];

    let out = fuzzy_match("feat", &items, 50);

    // Positions into `items`, best first: a prefix, then a substring.
    assert_eq!(out, vec![(1, 1.0), (0, 3.0)]);
}

/// Work items are titles, not paths: a typo has to be forgiven per word,
/// as it is per path segment in a branch.
#[test]
fn a_typo_matches_one_word_of_a_title() {
    let items = [Entry("fix login bug"), Entry("main")];

    let out = fuzzy_match("logn", &items, 50);

    // One edit away from the whole word `login`.
    assert_eq!(out, vec![(0, 4.1)]);
}

#[test]
fn a_word_inside_a_title_starts_at_a_word_boundary() {
    let items = [Entry("relogin"), Entry("fix login bug")];

    let out = fuzzy_match("login", &items, 50);

    // After a space ranks as a word boundary, ahead of mid-word.
    assert_eq!(out, vec![(1, 2.0), (0, 3.0)]);
}

#[test]
fn a_remote_branch_also_matches_by_its_local_name() {
    let remotes = vec!["origin".to_string()];
    let items = [
        BranchEntry::new("origin/feat", &remotes),
        BranchEntry::new("feat/login", &remotes),
    ];

    let out = fuzzy_match("feat", &items, 50);

    // Exact on the local name, then a prefix of the full name.
    assert_eq!(out, vec![(0, 0.5), (1, 1.0)]);
}

#[test]
fn highlight_range_finds_literal() {
    assert_eq!(highlight_range("ain", "main"), Some((1, 4)));
    assert_eq!(highlight_range("zzz", "main"), None);
}
