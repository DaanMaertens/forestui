//! Fuzzy branch search: score branches against a query, best first.

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
pub fn strip_remote_prefix<'a>(branch: &'a str, remotes: &[String]) -> &'a str {
    if let Some((prefix, rest)) = branch.split_once('/')
        && remotes.iter().any(|r| r == prefix)
    {
        return rest;
    }
    branch
}

/// Score a branch against a query. Lower is better; `None` means no match.
///
/// Scoring tiers (identical to the Textual implementation):
/// `0.0` exact, `0.5` exact on local name, `1.0` prefix on full name,
/// `1.5` prefix on local name, `2.0` substring at a word boundary,
/// `3.0` substring anywhere, `4.0+` fuzzy (Levenshtein) on path segments.
fn match_score(query: &str, branch: &str, remotes: &[String]) -> Option<f64> {
    let q = query.to_lowercase();
    let b = branch.to_lowercase();

    if q.is_empty() {
        return Some(0.0);
    }
    if q == b {
        return Some(0.0);
    }

    let local = strip_remote_prefix(&b, remotes).to_string();

    if q == local {
        return Some(0.5);
    }
    if b.starts_with(&q) {
        return Some(1.0);
    }
    if local.starts_with(&q) {
        return Some(1.5);
    }

    for target in [&b, &local] {
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

        for seg in b.split(['/', '-', '_', '.']) {
            if seg.is_empty() {
                continue;
            }
            let seg_chars: Vec<char> = seg.chars().collect();

            let dist = levenshtein_distance(&q_chars, &seg_chars);
            if dist <= threshold {
                let score = 4.0 + dist as f64 * 0.1;
                best = Some(best.map_or(score, |b: f64| b.min(score)));
            }

            if seg_chars.len() > q_chars.len() {
                let prefix: Vec<char> = seg_chars[..q_chars.len()].to_vec();
                let prefix_dist = levenshtein_distance(&q_chars, &prefix);
                if prefix_dist <= threshold {
                    let score = 4.5 + prefix_dist as f64 * 0.1;
                    best = Some(best.map_or(score, |b: f64| b.min(score)));
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
    if query.trim().is_empty() {
        return branches
            .iter()
            .take(max_results)
            .map(|b| (b.clone(), 0.0))
            .collect();
    }

    let mut results: Vec<(String, f64)> = branches
        .iter()
        .filter_map(|b| match_score(query, b, remotes).map(|s| (b.clone(), s)))
        .collect();

    results.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    results.truncate(max_results);
    results
}

/// Byte range of the literal `query` inside `branch`, for highlighting.
pub fn highlight_range(query: &str, branch: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let idx = branch.to_lowercase().find(&query.to_lowercase())?;
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

    #[test]
    fn highlight_range_finds_literal() {
        assert_eq!(highlight_range("ain", "main"), Some((1, 4)));
        assert_eq!(highlight_range("zzz", "main"), None);
    }
}
