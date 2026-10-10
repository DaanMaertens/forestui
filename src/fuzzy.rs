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

/// What splits a text into words: path separators for branches, spaces for
/// titles.
const WORD_SEPARATORS: [char; 5] = ['/', '-', '_', '.', ' '];

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
                    .is_some_and(|c| WORD_SEPARATORS.contains(&c));
            return Some(if boundary { 2.0 } else { 3.0 });
        }
    }

    let q_chars: Vec<char> = q.chars().collect();
    if q_chars.len() >= 2 {
        let mut best: Option<f64> = None;
        let threshold = q_chars.len().div_ceil(3).max(1);

        for seg in t.split(WORD_SEPARATORS) {
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
mod tests;
