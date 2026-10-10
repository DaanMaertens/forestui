//! Fuzzy search: score a list's entries against a query, best first.

/// Anything the fuzzy matcher can search.
pub trait Searchable {
    /// The text the query is matched against.
    fn search_text(&self) -> &str;

    /// A shorter form that also counts as an exact or prefix hit, e.g. a
    /// branch without its `origin/`. Most entries have none.
    fn short_text(&self) -> Option<&str> {
        None
    }
}

/// A branch, with its remote prefix stripped once up front rather than on
/// every keystroke.
#[derive(Debug, Clone)]
pub struct BranchEntry {
    name: String,
    /// `None` when the branch has no remote prefix.
    local: Option<String>,
}

impl BranchEntry {
    pub fn new(name: &str, remotes: &[String]) -> Self {
        // Stripped from the lowercased name, as the matcher always did: so
        // `Origin/x` counts as a branch of remote `origin`, while a remote
        // spelled `Origin` never strips.
        let lower = name.to_lowercase();
        let stripped = strip_remote_prefix(&lower, remotes);
        let local = (stripped != lower).then(|| stripped.to_string());
        Self {
            name: name.to_string(),
            local,
        }
    }
}

impl Searchable for BranchEntry {
    fn search_text(&self) -> &str {
        &self.name
    }

    fn short_text(&self) -> Option<&str> {
        self.local.as_deref()
    }
}

/// Match any searchable list against a query. Returns positions into
/// `items` with their scores, best first.
pub fn fuzzy_match<T: Searchable>(
    query: &str,
    items: &[T],
    max_results: usize,
) -> Vec<(usize, f64)> {
    if query.trim().is_empty() {
        return (0..items.len().min(max_results))
            .map(|i| (i, 0.0))
            .collect();
    }

    let mut results: Vec<(usize, f64)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            match_score(query, item.search_text(), item.short_text()).map(|s| (i, s))
        })
        .collect();

    let name = |i: usize| items[i].search_text().to_lowercase();
    results.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| name(a.0).cmp(&name(b.0)))
    });
    results.truncate(max_results);
    results
}

fn levenshtein_distance(s1: &[char], s2: &[char]) -> usize {
    if s1.len() < s2.len() {
        return levenshtein_distance(s2, s1);
    }
    if s2.is_empty() {
        return s1.len();
    }

    let mut prev_row: Vec<usize> = (0..=s2.len()).collect();
    for &c1 in s1 {
        let mut curr_row = Vec::with_capacity(s2.len() + 1);
        curr_row.push(prev_row[0] + 1);
        for (j, &c2) in s2.iter().enumerate() {
            let insertions = prev_row[j + 1] + 1;
            let deletions = curr_row[j] + 1;
            let substitutions = prev_row[j] + usize::from(c1 != c2);
            curr_row.push(insertions.min(deletions).min(substitutions));
        }
        prev_row = curr_row;
    }
    prev_row[s2.len()]
}

/// Strip a remote prefix (e.g. `origin/`) using the repository's real remotes.
fn strip_remote_prefix<'a>(branch: &'a str, remotes: &[String]) -> &'a str {
    if let Some((prefix, rest)) = branch.split_once('/')
        && remotes.iter().any(|r| r == prefix)
    {
        return rest;
    }
    branch
}

/// Score a text against a query. Lower is better; `None` means no match.
/// `short` is the entry's short form; without one, the full text stands in.
///
/// Scoring tiers (identical to the Textual implementation):
/// `0.0` exact, `0.5` exact on the short form, `1.0` prefix on the full text,
/// `1.5` prefix on the short form, `2.0` substring at a word boundary,
/// `3.0` substring anywhere, `4.0+` fuzzy (Levenshtein) on words.
fn match_score(query: &str, text: &str, short: Option<&str>) -> Option<f64> {
    let q = query.to_lowercase();
    let t = text.to_lowercase();

    if q.is_empty() {
        return Some(0.0);
    }
    if q == t {
        return Some(0.0);
    }

    let short = short.map_or_else(|| t.clone(), str::to_lowercase);

    if q == short {
        return Some(0.5);
    }
    if t.starts_with(&q) {
        return Some(1.0);
    }
    if short.starts_with(&q) {
        return Some(1.5);
    }

    for target in [&t, &short] {
        if let Some(idx) = target.find(&q) {
            let boundary = idx == 0
                || target[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '/' | '-' | '_' | '.'));
            return Some(if boundary { 2.0 } else { 3.0 });
        }
    }

    let q_chars: Vec<char> = q.chars().collect();
    if q_chars.len() >= 2 {
        let mut best: Option<f64> = None;
        let threshold = q_chars.len().div_ceil(3).max(1);

        for seg in t.split(['/', '-', '_', '.']) {
            if seg.is_empty() {
                continue;
            }
            let seg_chars: Vec<char> = seg.chars().collect();

            let dist = levenshtein_distance(&q_chars, &seg_chars);
            if dist <= threshold {
                let score = 4.0 + dist as f64 * 0.1;
                best = Some(best.map_or(score, |best: f64| best.min(score)));
            }

            if seg_chars.len() > q_chars.len() {
                let prefix: Vec<char> = seg_chars[..q_chars.len()].to_vec();
                let prefix_dist = levenshtein_distance(&q_chars, &prefix);
                if prefix_dist <= threshold {
                    let score = 4.5 + prefix_dist as f64 * 0.1;
                    best = Some(best.map_or(score, |best: f64| best.min(score)));
                }
            }
        }
        return best;
    }

    None
}

/// Match branches against a query, best first.
pub fn fuzzy_match_branches(
    query: &str,
    branches: &[String],
    remotes: &[String],
    max_results: usize,
) -> Vec<(String, f64)> {
    let entries: Vec<BranchEntry> = branches
        .iter()
        .map(|b| BranchEntry::new(b, remotes))
        .collect();
    fuzzy_match(query, &entries, max_results)
        .into_iter()
        .map(|(i, score)| (branches[i].clone(), score))
        .collect()
}

/// Byte range of the literal `query` inside `text`, for highlighting.
pub fn highlight_range(query: &str, text: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let idx = text.to_lowercase().find(&query.to_lowercase())?;
    Some((idx, idx + query.len()))
}

#[cfg(test)]
mod tests {
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
}
