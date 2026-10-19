//! Start work's pure rules: the work modes, how a work item names its
//! worktree and branch, and the entry a work item is searched by.

use crate::fuzzy::Searchable;
use crate::models::{
    GitHubIssue, GitHubPullRequest, IssueFilter, ItemState, Naming, StartWorkSettings,
    WorkItemLink, Worktree,
};
use crate::prompt::Template;

/// The longest slug a title contributes to a name, so a long title cannot
/// produce a directory or branch name nobody wants to type.
const SLUG_LIMIT: usize = 40;

/// The kind of work about to start, which decides what the item list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMode {
    /// Every open issue. A feature label decides only how a picked one
    /// starts — see [`IssueKind`] — not which list it is in.
    Issue,
    /// A Claude session for work that is no issue or PR: its work item is a
    /// session prompt the user types, which exists nowhere but here.
    Session,
    PrReview,
    /// A plain worktree: no work item, no prompt, no Claude. It has no item
    /// list; its step is a form for the worktree and its branch.
    Worktree,
}

impl WorkMode {
    /// Every mode, in the order the mode step lists them.
    pub const ALL: [WorkMode; 4] = [
        WorkMode::Issue,
        WorkMode::Session,
        WorkMode::PrReview,
        WorkMode::Worktree,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WorkMode::Issue => "issue",
            WorkMode::Session => "single session",
            WorkMode::PrReview => "pr-review",
            WorkMode::Worktree => "worktree",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            WorkMode::Issue => "Work on an issue: feature, bug or chore",
            WorkMode::Session => "A Claude session for anything else",
            WorkMode::PrReview => "Review a pull request",
            WorkMode::Worktree => "A plain worktree: new or existing branch, no Claude",
        }
    }
}

/// How a PR is reviewed, which follows from who opened it — there is no
/// switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    /// Someone else's PR: review and report, change nothing.
    Others,
    /// My own PR: list findings, wait for a go-ahead, then fix.
    Own,
}

impl ReviewKind {
    pub fn label(self) -> &'static str {
        match self {
            ReviewKind::Others => "Review someone else's PR",
            ReviewKind::Own => "Review my own PR",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ReviewKind::Others => "Review and report; no code changes",
            ReviewKind::Own => "List findings, wait for your go-ahead, then fix",
        }
    }

    fn template(self) -> Template {
        match self {
            ReviewKind::Others => Template::PrReviewOthers,
            ReviewKind::Own => Template::PrReviewOwn,
        }
    }
}

/// The review kind of a PR opened by `author`, as seen by `viewer`, the
/// login `gh` is authenticated as. GitHub logins ignore case.
///
/// An unknown viewer makes every PR someone else's: that review changes no
/// code, so guessing wrong that way costs nothing, while the other way
/// would have Claude editing a stranger's branch.
pub fn review_kind(author: &str, viewer: Option<&str>) -> ReviewKind {
    match viewer {
        Some(viewer) if !viewer.is_empty() && viewer.eq_ignore_ascii_case(author) => {
            ReviewKind::Own
        }
        _ => ReviewKind::Others,
    }
}

/// Whether an issue carries one of `labels`. GitHub treats label names
/// case-insensitively, so `Enhancement` counts as `enhancement`.
fn has_label(issue: &GitHubIssue, labels: &[impl AsRef<str>]) -> bool {
    issue.labels.iter().any(|label| {
        labels
            .iter()
            .any(|wanted| label.name.eq_ignore_ascii_case(wanted.as_ref()))
    })
}

/// Whether `issue` is among the issues `filter` lists, by the feature and
/// bug labels `settings` give. An issue with both is a feature and a bug.
pub fn in_filter(issue: &GitHubIssue, filter: IssueFilter, settings: &StartWorkSettings) -> bool {
    let feature = has_label(issue, &settings.feature_labels);
    let bug = has_label(issue, &settings.bug_labels);
    match filter {
        IssueFilter::All => true,
        IssueFilter::Features => feature,
        IssueFilter::Bugs => bug,
        IssueFilter::Other => !feature && !bug,
    }
}

/// How a picked issue starts: which naming names its worktree and branch,
/// and which prompt template opens its session. Both kinds are listed
/// together in the `issue` work mode; the kind only says how to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Issue,
    Feature,
}

impl IssueKind {
    /// What the confirm screen calls the work item: "Feature #7".
    pub fn label(self) -> &'static str {
        match self {
            IssueKind::Issue => "Issue",
            IssueKind::Feature => "Feature",
        }
    }

    fn naming(self, settings: &StartWorkSettings) -> &Naming {
        match self {
            IssueKind::Issue => &settings.issue,
            IssueKind::Feature => &settings.feature,
        }
    }

    fn template(self) -> Template {
        match self {
            IssueKind::Issue => Template::Issue,
            IssueKind::Feature => Template::Feature,
        }
    }
}

/// The kind an issue starts as: a feature if it carries a feature label —
/// which wins over anything else it carries, so a `bug` + `enhancement`
/// issue is a feature — and an issue otherwise.
pub fn issue_kind(issue: &GitHubIssue, feature_labels: &[impl AsRef<str>]) -> IssueKind {
    if has_label(issue, feature_labels) {
        IssueKind::Feature
    } else {
        IssueKind::Issue
    }
}

/// The worktree and branch a work item starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNames {
    pub worktree: String,
    pub branch: String,
}

/// Name the worktree and branch for an issue, by the naming `settings`
/// give its kind.
///
/// By default the worktree carries the kind, the number and a slug of the
/// title, so the sidebar shows what kind of work it holds, and the branch is
/// the worktree name under the kind's conventional prefix.
pub fn issue_names(
    settings: &StartWorkSettings,
    kind: IssueKind,
    number: i64,
    title: &str,
) -> WorktreeNames {
    let naming = kind.naming(settings);
    let worktree = numbered_name(&naming.name_pattern, number, title);
    let branch = fill_pattern(&naming.branch_pattern, Some(number), &title_slug(title));
    WorktreeNames {
        branch: prefixed_branch(naming, branch, &worktree),
        worktree,
    }
}

/// Name the worktree for a PR to review, by the `pr-review` naming. It
/// checks out the PR's own branch, so only the worktree name is used.
pub fn pull_request_names(settings: &StartWorkSettings, number: i64, title: &str) -> WorktreeNames {
    let worktree = numbered_name(&settings.pr_review.name_pattern, number, title);
    WorktreeNames {
        branch: worktree.clone(),
        worktree,
    }
}

fn numbered_name(pattern: &str, number: i64, title: &str) -> String {
    let filled = fill_pattern(pattern, Some(number), &title_slug(title));
    // A pattern that fills to nothing would name every item alike.
    if filled.is_empty() {
        number.to_string()
    } else {
        filled
    }
}

/// A name pattern with `{number}` and `{slug}` filled in, kept to what a
/// worktree directory may be called: letters, digits, `-` and `_`. A
/// placeholder that fills to nothing leaves no doubled or dangling `-`
/// behind, so `issue-{number}-{slug}` on an empty slug is `issue-<n>`.
fn fill_pattern(pattern: &str, number: Option<i64>, slug: &str) -> String {
    let number = number.map(|n| n.to_string()).unwrap_or_default();
    let filled = pattern.replace("{number}", &number).replace("{slug}", slug);
    let mut out = String::with_capacity(filled.len());
    for ch in filled.chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch
        } else {
            '-'
        };
        if !(ch == '-' && (out.is_empty() || out.ends_with('-'))) {
            out.push(ch);
        }
    }
    out.trim_end_matches('-').to_string()
}

/// A title as a name fragment: lowercase ASCII letters and digits, every other
/// run of characters collapsed to one `-`, capped at [`SLUG_LIMIT`].
pub fn title_slug(title: &str) -> String {
    let slug = crate::util::slugify(title);
    let capped: String = slug.chars().take(SLUG_LIMIT).collect();
    capped.trim_matches('-').to_string()
}

/// How many words of a session prompt its names are made from. A prompt is
/// a sentence or more, not a title, and a name built from all of it would
/// be cut off mid-word by the cap anyway.
const SESSION_WORDS: usize = 6;

/// How a session with nothing to name it after is named instead: the
/// minute it was started, which is unique enough and sorts.
const SESSION_TIMESTAMP: &str = "%Y%m%d-%H%M";

/// Name the worktree and branch for a session, by the `session` naming —
/// `session-<slug>` on `session/<slug>` by default — from the prompt's first
/// words, or from `now` when they hold nothing a name can be made of: a
/// blank prompt is a fresh worktree to talk to Claude in, and still needs a
/// name of its own. `None` only when the pattern fills to nothing: a bare
/// prefix would be shared by every session. A pattern without `{slug}`
/// names them all alike too; the clash question is what catches that.
pub fn session_names(
    settings: &StartWorkSettings,
    now: chrono::NaiveDateTime,
    prompt: &str,
) -> Option<WorktreeNames> {
    let first_words: Vec<&str> = prompt.split_whitespace().take(SESSION_WORDS).collect();
    let slug = title_slug(&first_words.join(" "));
    let slug = if slug.is_empty() {
        now.format(SESSION_TIMESTAMP).to_string()
    } else {
        slug
    };
    session_names_for_slug(settings, &slug)
}

/// Name the worktree and branch for a session from a name suggestion's
/// reply: its first non-empty line — whatever prose a model adds after the
/// name is not the name — slugged and capped like a title, through the
/// `session` naming. `None` when that line holds nothing a name can be made
/// of, so the first-words names stay.
pub fn suggested_session_names(settings: &StartWorkSettings, reply: &str) -> Option<WorktreeNames> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    session_names_for_slug(settings, &title_slug(line))
}

fn session_names_for_slug(settings: &StartWorkSettings, slug: &str) -> Option<WorktreeNames> {
    if slug.is_empty() {
        return None;
    }
    let naming = &settings.session;
    let worktree = fill_pattern(&naming.name_pattern, None, slug);
    if worktree.is_empty() {
        return None;
    }
    let branch = fill_pattern(&naming.branch_pattern, None, slug);
    Some(WorktreeNames {
        branch: prefixed_branch(naming, branch, &worktree),
        worktree,
    })
}

/// The filled branch pattern under the mode's prefix — or the worktree name,
/// when the pattern filled to nothing: a bare prefix would be every item's
/// branch.
fn prefixed_branch(naming: &Naming, filled: String, worktree: &str) -> String {
    let name = if filled.is_empty() { worktree } else { &filled };
    format!("{}{name}", naming.branch_prefix)
}

/// `names` with `-<n>` added to both, for the first `n` from 2 that `taken`
/// does not refuse.
pub fn first_free_copy(
    names: &WorktreeNames,
    taken: impl Fn(&WorktreeNames) -> bool,
) -> WorktreeNames {
    (2..)
        .map(|n| WorktreeNames {
            worktree: format!("{}-{n}", names.worktree),
            branch: format!("{}-{n}", names.branch),
        })
        .find(|copy| !taken(copy))
        .unwrap_or_else(|| names.clone())
}

/// The worktree that already holds `names`' worktree name or branch — either
/// one would make the create fail.
pub fn clash<'a>(names: &WorktreeNames, worktrees: &'a [Worktree]) -> Option<&'a Worktree> {
    worktrees
        .iter()
        .find(|w| w.name == names.worktree || w.branch == names.branch)
}

/// What a worktree is started for: an issue, a pull request to review, or a
/// session prompt typed in by the user, which exists nowhere but here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkItem {
    Issue {
        number: i64,
        title: String,
        kind: IssueKind,
    },
    PullRequest {
        number: i64,
        title: String,
        author: String,
        /// The branch the PR is opened from, which `gh pr checkout` checks out.
        head_branch: String,
        /// Opened from a fork: `head_branch` is the fork's name for it.
        from_fork: bool,
        review: ReviewKind,
    },
    Session {
        prompt: String,
    },
}

impl WorkItem {
    pub fn from_issue(issue: &GitHubIssue, kind: IssueKind) -> Self {
        WorkItem::Issue {
            number: issue.number,
            title: issue.title.clone(),
            kind,
        }
    }

    /// A PR, its review kind settled against `viewer` now, once.
    pub fn from_pull_request(pr: &GitHubPullRequest, viewer: Option<&str>) -> Self {
        WorkItem::PullRequest {
            number: pr.number,
            title: pr.title.clone(),
            author: pr.author.login.clone(),
            head_branch: pr.head_ref_name.clone(),
            from_fork: pr.is_cross_repository,
            review: review_kind(&pr.author.login, viewer),
        }
    }

    /// The record the worktree keeps of it.
    pub fn link(&self) -> WorkItemLink {
        match self {
            WorkItem::Issue { number, .. } => WorkItemLink::Issue { number: *number },
            WorkItem::PullRequest { number, .. } => WorkItemLink::PullRequest { number: *number },
            WorkItem::Session { prompt } => WorkItemLink::Session {
                prompt: prompt.clone(),
            },
        }
    }

    /// The template its first prompt is filled from. An issue's depends on
    /// its kind; a PR's on its review kind.
    pub fn template(&self) -> Template {
        match self {
            WorkItem::Issue { kind, .. } => kind.template(),
            WorkItem::PullRequest { review, .. } => review.template(),
            WorkItem::Session { prompt } if prompt.trim().is_empty() => Template::SessionBlank,
            WorkItem::Session { .. } => Template::Session,
        }
    }
}

/// Where the worktree of a sub-issue's parent issue comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentWorktree {
    /// The worktrees linked to the parent issue. One is used as it is;
    /// with several, `chosen` stays `None` until the user picks one.
    Existing {
        candidates: Vec<ParentCandidate>,
        chosen: Option<usize>,
    },
    /// None is linked to it yet: one is created under these names first.
    Create(WorktreeNames),
}

/// A worktree linked to a parent issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentCandidate {
    pub name: String,
    pub branch: String,
    pub path: String,
}

impl ParentWorktree {
    /// The parent's branch, which the sub-issue's stems from. `None` while
    /// several worktrees wait to be picked from.
    pub fn branch(&self) -> Option<&str> {
        match self {
            ParentWorktree::Existing { candidates, chosen } => chosen
                .and_then(|index| candidates.get(index))
                .map(|candidate| candidate.branch.as_str()),
            ParentWorktree::Create(names) => Some(&names.branch),
        }
    }

    /// The worktree in use, if one exists and is chosen.
    pub fn chosen(&self) -> Option<&ParentCandidate> {
        match self {
            ParentWorktree::Existing { candidates, chosen } => {
                chosen.and_then(|index| candidates.get(index))
            }
            ParentWorktree::Create(_) => None,
        }
    }

    /// Whether there is a choice to make: more than one worktree.
    pub fn is_choice(&self) -> bool {
        matches!(self, ParentWorktree::Existing { candidates, .. } if candidates.len() > 1)
    }

    /// Pick the next worktree, from the first when none is picked yet.
    pub fn pick_next(&mut self) {
        if let ParentWorktree::Existing { candidates, chosen } = self
            && !candidates.is_empty()
        {
            *chosen = Some(chosen.map_or(0, |index| (index + 1) % candidates.len()));
        }
    }
}

/// The worktree of parent issue `parent` among `worktrees`: the ones whose
/// work item link is the parent issue, or — with none — one to create
/// under `names`, the names Start work gives the parent issue by default.
pub fn parent_worktree(
    worktrees: &[Worktree],
    parent: i64,
    names: WorktreeNames,
) -> ParentWorktree {
    let link = WorkItemLink::Issue { number: parent };
    let candidates: Vec<ParentCandidate> = worktrees
        .iter()
        .filter(|worktree| worktree.work_item.as_ref() == Some(&link))
        .map(|worktree| ParentCandidate {
            name: worktree.name.clone(),
            branch: worktree.branch.clone(),
            path: worktree.path.clone(),
        })
        .collect();
    match candidates.len() {
        0 => ParentWorktree::Create(names),
        1 => ParentWorktree::Existing {
            candidates,
            chosen: Some(0),
        },
        _ => ParentWorktree::Existing {
            candidates,
            chosen: None,
        },
    }
}

/// What a finished check saw of a worktree's work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// GitHub's state for an issue or pull request.
    State(ItemState),
    /// A session's branch, against the remote default branch.
    Branch {
        /// The branch tip is contained in the default branch.
        merged: bool,
        /// The branch has commits beyond the one it was created from.
        has_commits: bool,
    },
}

/// Whether a worktree's work item is done: an issue closed, a pull request
/// merged or closed, a session's branch merged. `None` when the
/// observation is not about this kind of link and so decides nothing.
///
/// A session's branch needs commits of its own: one just created from the
/// default branch is already contained in it, and that is a worktree whose
/// work has not started, not one whose work is done.
pub fn is_finished(link: &WorkItemLink, observed: Observed) -> Option<bool> {
    match (link, observed) {
        (WorkItemLink::Issue { .. } | WorkItemLink::PullRequest { .. }, Observed::State(state)) => {
            Some(state != ItemState::Open)
        }
        (
            WorkItemLink::Session { .. },
            Observed::Branch {
                merged,
                has_commits,
            },
        ) => Some(merged && has_commits),
        _ => None,
    }
}

/// Whether a sub-issue's worktree is done, from its issue's `state` and
/// whether its pull request into the parent issue's branch is merged:
/// either one finishes it, since its issue normally stays open until the
/// parent's pull request lands. `None` when neither is known to be done
/// and one could not be told.
pub fn is_sub_issue_finished(
    state: Option<ItemState>,
    merged_into_parent: Option<bool>,
) -> Option<bool> {
    let closed = state.map(|state| state != ItemState::Open);
    match (closed, merged_into_parent) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// One row of the item picker.
#[derive(Debug, Clone)]
pub enum ItemEntry {
    Issue(IssueEntry),
    PullRequest(PullRequestEntry),
}

impl ItemEntry {
    /// The worktree already started for the row's item, if any.
    pub fn worktree(&self) -> Option<&str> {
        match self {
            ItemEntry::Issue(entry) => entry.worktree.as_deref(),
            ItemEntry::PullRequest(entry) => entry.worktree.as_deref(),
        }
    }
}

impl Searchable for ItemEntry {
    fn search_text(&self) -> &str {
        match self {
            ItemEntry::Issue(entry) => entry.search_text(),
            ItemEntry::PullRequest(entry) => entry.search_text(),
        }
    }
}

/// The rows `pr-review` lists: every PR, each with the worktree `linked`
/// says it was started in.
pub fn pull_request_entries(
    prs: &[GitHubPullRequest],
    linked: impl Fn(i64) -> Option<String>,
) -> Vec<ItemEntry> {
    prs.iter()
        .map(|pr| ItemEntry::PullRequest(PullRequestEntry::new(pr.clone(), linked(pr.number))))
        .collect()
}

/// A PR as the item picker searches it: by number and title, like an issue.
#[derive(Debug, Clone)]
pub struct PullRequestEntry {
    pub pr: GitHubPullRequest,
    /// The name of the worktree already started for this PR, if any.
    pub worktree: Option<String>,
    text: String,
}

impl PullRequestEntry {
    pub fn new(pr: GitHubPullRequest, worktree: Option<String>) -> Self {
        let text = format!("#{} {}", pr.number, pr.title);
        Self { pr, worktree, text }
    }
}

impl Searchable for PullRequestEntry {
    fn search_text(&self) -> &str {
        &self.text
    }
}

/// Whether a list shows `issue`: always, unless it is blocked and blocked
/// issues are hidden. Only a view filter — the cache and duplicate
/// detection see every issue.
pub fn is_shown(issue: &GitHubIssue, show_blocked: bool) -> bool {
    show_blocked || !issue.is_blocked()
}

/// Whether a list under `filter` shows `issue`: the filter first, then
/// blocked-hiding.
fn is_listed(
    issue: &GitHubIssue,
    filter: IssueFilter,
    settings: &StartWorkSettings,
    show_blocked: bool,
) -> bool {
    in_filter(issue, filter, settings) && is_shown(issue, show_blocked)
}

/// How many of the issues in `filter` a list leaves out for being blocked:
/// one the filter already leaves out is not missing from the list.
pub fn hidden_blocked(
    issues: &[GitHubIssue],
    filter: IssueFilter,
    settings: &StartWorkSettings,
    show_blocked: bool,
) -> usize {
    issues
        .iter()
        .filter(|issue| in_filter(issue, filter, settings) && !is_shown(issue, show_blocked))
        .count()
}

/// What a list says about the blocked issues it hides, so a missing issue is
/// not a mystery. `None` when it hides none.
pub fn hidden_blocked_text(hidden: usize) -> Option<String> {
    (hidden > 0).then(|| format!("{hidden} blocked hidden"))
}

/// How many blockers the marker names before it only counts the rest.
const MARKER_BLOCKERS: usize = 3;

/// The marker a shown blocked issue carries, naming what blocks it. `None`
/// for an issue nothing open blocks.
pub fn blocked_marker(issue: &GitHubIssue) -> Option<String> {
    let blockers = issue.open_blockers();
    if blockers.is_empty() {
        return None;
    }
    let named: Vec<String> = blockers
        .iter()
        .take(MARKER_BLOCKERS)
        .map(|number| format!("#{number}"))
        .collect();
    let mut marker = format!("blocked by {}", named.join(", "));
    if blockers.len() > MARKER_BLOCKERS {
        marker.push_str(&format!(" +{}", blockers.len() - MARKER_BLOCKERS));
    }
    Some(marker)
}

/// The marker a sub-issue carries, naming its parent: `↳ #12`, or with the
/// repository for a parent in another one. `None` for an issue with none.
pub fn parent_marker(issue: &GitHubIssue) -> Option<String> {
    Some(format!("↳ {}", issue.parent_reference()?))
}

/// The rows `issue` lists: the open issues in `filter` — less the blocked
/// ones, unless `show_blocked` — each with the worktree `linked` says it was
/// started in.
pub fn issue_entries(
    issues: &[GitHubIssue],
    filter: IssueFilter,
    settings: &StartWorkSettings,
    show_blocked: bool,
    linked: impl Fn(i64) -> Option<String>,
) -> Vec<ItemEntry> {
    issues
        .iter()
        .filter(|issue| is_listed(issue, filter, settings, show_blocked))
        .map(|issue| {
            let worktree = linked(issue.number);
            ItemEntry::Issue(IssueEntry::new(issue.clone(), worktree))
        })
        .collect()
}

/// An issue as the item picker searches it: by number and title together, so
/// typing either finds it.
#[derive(Debug, Clone)]
pub struct IssueEntry {
    pub issue: GitHubIssue,
    /// The name of the worktree already started for this issue, if any. Shown
    /// as a marker only; it is not searched.
    pub worktree: Option<String>,
    text: String,
}

impl IssueEntry {
    pub fn new(issue: GitHubIssue, worktree: Option<String>) -> Self {
        let text = format!("#{} {}", issue.number, issue.title);
        Self {
            issue,
            worktree,
            text,
        }
    }
}

impl Searchable for IssueEntry {
    fn search_text(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests;
