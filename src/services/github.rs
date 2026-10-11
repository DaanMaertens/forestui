//! GitHub integration via the `gh` CLI.

use crate::models::{GitHubIssue, GitHubPullRequest, ItemState, Scope};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tokio::process::Command;

pub const CACHE_TTL_SECONDS: i64 = 300;

/// Result of `gh auth status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    Authenticated,
    NotAuthenticated,
    NotInstalled,
}

impl AuthStatus {
    /// Short text shown in the sidebar header.
    pub fn display(&self, username: Option<&str>) -> String {
        match self {
            AuthStatus::Authenticated => match username {
                Some(u) => format!("ok ({u})"),
                None => "ok".to_string(),
            },
            AuthStatus::NotAuthenticated => "unauth'd".to_string(),
            AuthStatus::NotInstalled => "missing".to_string(),
        }
    }
}

struct Cached {
    issues: Vec<GitHubIssue>,
    fetched_at: DateTime<Utc>,
}

/// The cache is keyed by the repository *path* and the scope: a list
/// fetched for the other scope answers a different question, so it is never
/// served as this one, and the two are held side by side because the
/// prefetch warms the setting's scope while an open Start work can be
/// listing the other.
///
/// The path is the one identifier the app
/// holds synchronously. Keying by `owner/repo` made every lookup pay a
/// `gh repo view` subprocess before it could even ask the cache, which is why
/// the issues list used to flash "Loading…" on every selection change (#29).
///
/// Its own type rather than fields of [`State`] so tests can each hold one:
/// the process-wide instance is shared by every test thread, and a test that
/// read the generation could see another test's invalidation bump it before
/// its own store landed.
#[derive(Default)]
struct IssueCache {
    entries: HashMap<(String, Scope), Cached>,
    /// Bumped by every invalidation. A fetch that started before an
    /// invalidation must not store its result afterwards — it would re-stamp
    /// pre-invalidation data as fresh and silently undo the refresh.
    generation: u64,
}

impl IssueCache {
    /// `Some((issues, fresh))` when a fetch has completed, `None` otherwise.
    fn peek(&self, path: &str, scope: Scope) -> Option<(Vec<GitHubIssue>, bool)> {
        let cached = self.entries.get(&(path.to_string(), scope))?;
        let fresh = (Utc::now() - cached.fetched_at).num_seconds() < CACHE_TTL_SECONDS;
        Some((cached.issues.clone(), fresh))
    }

    /// Store a fetch result unless an invalidation landed while it ran.
    /// `fresh` decides the stamp: `false` back-dates the entry to the TTL
    /// boundary so it renders instantly but re-checks on the next visit.
    fn store_if_current(
        &mut self,
        generation: u64,
        path: &str,
        scope: Scope,
        issues: &[GitHubIssue],
        fresh: bool,
    ) {
        if self.generation != generation {
            return;
        }
        let fetched_at = if fresh {
            Utc::now()
        } else {
            Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECONDS)
        };
        self.entries.insert(
            (path.to_string(), scope),
            Cached {
                issues: issues.to_vec(),
                fetched_at,
            },
        );
    }

    /// Drop one path's entry. Deliberately not a clear-everything: nuking the
    /// whole cache made the next selection change on every other repository a
    /// guaranteed cold miss, which was half of #29.
    fn invalidate(&mut self, path: &str) {
        self.entries.retain(|(cached, _), _| cached != path);
        // Fence out any fetch already in flight for the old contents.
        self.generation += 1;
    }
}

#[derive(Default)]
struct State {
    issues: IssueCache,
    auth: Option<(AuthStatus, Option<String>)>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// Exit code reserved for a process that died to a signal.
///
/// It cannot be `0` (success), `-1` (reserved for "gh is not installed"), or
/// `1` — `gh auth status` returns 1 for "not logged in", and conflating the two
/// let one killed probe pin the whole session to "unauth'd".
pub const KILLED: i32 = -2;

fn exit_code(status: std::process::ExitStatus) -> i32 {
    crate::util::exit_code(status, KILLED)
}

/// Run a `gh` command. Exit code `-1` means the binary is missing.
async fn run_gh(args: &[&str], cwd: Option<&Path>) -> (i32, String, String) {
    let mut cmd = Command::new("gh");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    match cmd.output().await {
        Ok(output) => (
            exit_code(output.status),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(_) => (-1, String::new(), "gh not found".to_string()),
    }
}

/// Authentication status and username. Cached for the lifetime of the process,
/// matching the Textual build.
pub async fn get_auth_status() -> (AuthStatus, Option<String>) {
    if let Ok(guard) = state().lock()
        && let Some(cached) = &guard.auth
    {
        return cached.clone();
    }

    let (code, _out, _err) = run_gh(&["auth", "status"], None).await;
    if code == KILLED {
        // A probe that was killed answered nothing. Caching that as an answer
        // would leave the sidebar reading "unauth'd" and the issues list empty
        // for the rest of the session, with no path back short of a restart.
        return (AuthStatus::NotAuthenticated, None);
    }
    let result = if code == -1 {
        (AuthStatus::NotInstalled, None)
    } else if code == 0 {
        let (code, stdout, _) = run_gh(&["api", "user", "--jq", ".login"], None).await;
        let username = if code == 0 && !stdout.is_empty() {
            Some(stdout)
        } else {
            None
        };
        (AuthStatus::Authenticated, username)
    } else {
        (AuthStatus::NotAuthenticated, None)
    };

    if let Ok(mut guard) = state().lock() {
        guard.auth = Some(result.clone());
    }
    result
}

/// `(owner, repo)` for a path, or `None` when it is not a GitHub repository.
pub async fn get_repo_info(path: &str) -> Option<(String, String)> {
    let (code, stdout, _) = run_gh(
        &["repo", "view", "--json", "owner,name"],
        Some(Path::new(path)),
    )
    .await;
    if code != 0 || stdout.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let owner = value.get("owner")?.get("login")?.as_str()?.to_string();
    let name = value.get("name")?.as_str()?.to_string();
    Some((owner, name))
}

/// Cached issues for a path without any I/O: `Some((issues, fresh))` when a
/// fetch has completed this run, `None` when nothing is cached. Safe to call
/// from the render thread — the lock is only ever held for map operations,
/// never across an await.
pub fn peek_issues(path: &str, scope: Scope) -> Option<(Vec<GitHubIssue>, bool)> {
    state().lock().ok()?.issues.peek(path, scope)
}

/// Open issues in `scope` — assigned to or opened by the current user, or
/// every one — newest first.
pub async fn list_issues(path: &str, limit: usize, scope: Scope) -> Vec<GitHubIssue> {
    // The cache answers before any subprocess runs; a warm hit costs nothing.
    // Freshness has exactly one definition, and it lives in `peek_issues`.
    if let Some((issues, true)) = peek_issues(path, scope) {
        return issues;
    }
    let generation = state()
        .lock()
        .map(|g| g.issues.generation)
        .unwrap_or_default();

    // Empty answers are cached too — a local-only repository or an
    // unauthenticated `gh` would otherwise re-flash "Loading…" and re-spawn
    // subprocesses on every visit — but as *already stale*: the panel renders
    // them instantly while the next visit silently re-checks, so a transient
    // failure (a killed probe, a network blip during `gh repo view`) never
    // masquerades as a confident 300-second "No issues found".
    let (auth, _) = get_auth_status().await;
    if auth != AuthStatus::Authenticated {
        return store_if_current(generation, path, scope, Vec::new(), false);
    }

    // Only the gate cares about owner/repo: a path that is not a GitHub
    // repository has no issues to list.
    if get_repo_info(path).await.is_none() {
        return store_if_current(generation, path, scope, Vec::new(), false);
    }

    let issues = fetch_issues(path, limit, scope).await;
    store_if_current(generation, path, scope, issues, true)
}

/// [`IssueCache::store_if_current`] on the shared cache, handing the issues
/// back so `list_issues` can return what it stored.
fn store_if_current(
    generation: u64,
    path: &str,
    scope: Scope,
    issues: Vec<GitHubIssue>,
    fresh: bool,
) -> Vec<GitHubIssue> {
    if let Ok(mut guard) = state().lock() {
        guard
            .issues
            .store_if_current(generation, path, scope, &issues, fresh);
    }
    issues
}

/// Seed the shared cache directly, for App tests that must not shell out to
/// `gh`. Tests of the cache itself use their own [`IssueCache`] instead.
#[cfg(test)]
pub fn seed_cache_for_test(
    path: &str,
    scope: Scope,
    issues: Vec<GitHubIssue>,
    fetched_at: DateTime<Utc>,
) {
    if let Ok(mut guard) = state().lock() {
        guard
            .issues
            .entries
            .insert((path.to_string(), scope), Cached { issues, fetched_at });
    }
}

/// The `gh issue list` filters that make up `scope`, one call each: mine is
/// assigned to me plus opened by me, and all is one unfiltered call.
fn issue_filters(scope: Scope) -> &'static [&'static [&'static str]] {
    match scope {
        Scope::Mine => &[&["--assignee", "@me"], &["--author", "@me"]],
        Scope::All => &[&[]],
    }
}

/// The `gh pr list` filters that make up `scope`, as [`issue_filters`]:
/// mine is a review requested from me plus opened by me.
fn pull_request_filters(scope: Scope) -> &'static [&'static [&'static str]] {
    match scope {
        Scope::Mine => &[&["--search", "review-requested:@me"], &["--author", "@me"]],
        Scope::All => &[&[]],
    }
}

/// The fields an issue is fetched with. `blockedBy` comes along in the same
/// call, so knowing which issues are blocked costs no `gh` call per issue.
const ISSUE_FIELDS: &str =
    "number,title,state,url,createdAt,updatedAt,author,assignees,labels,blockedBy";

/// [`ISSUE_FIELDS`] for a `gh` too old to know `blockedBy`, whose issues
/// then all count as unblocked.
const LEGACY_ISSUE_FIELDS: &str =
    "number,title,state,url,createdAt,updatedAt,author,assignees,labels";

/// Whether `gh` refused a call because it does not know `blockedBy`. `gh`
/// rejects the whole call over one unknown field, so without the retry this
/// enables an older `gh` would list no issues at all.
fn rejects_blocked_by(stderr: &str) -> bool {
    stderr.contains("Unknown JSON field") && stderr.contains("blockedBy")
}

async fn fetch_issues(path: &str, limit: usize, scope: Scope) -> Vec<GitHubIssue> {
    let limit_str = limit.to_string();

    let mut issues: Vec<GitHubIssue> = Vec::new();
    let mut seen: Vec<i64> = Vec::new();

    for filter in issue_filters(scope) {
        let mut args = vec!["issue", "list"];
        args.extend_from_slice(filter);
        args.extend_from_slice(&["--state", "open", "--limit", &limit_str, "--json"]);
        let (mut code, mut stdout, stderr) = run_gh(
            &[&args[..], &[ISSUE_FIELDS]].concat(),
            Some(Path::new(path)),
        )
        .await;
        if code != 0 && rejects_blocked_by(&stderr) {
            (code, stdout, _) = run_gh(
                &[&args[..], &[LEGACY_ISSUE_FIELDS]].concat(),
                Some(Path::new(path)),
            )
            .await;
        }
        if code != 0 || stdout.is_empty() {
            continue;
        }
        let parsed: Vec<GitHubIssue> = serde_json::from_str(&stdout).unwrap_or_default();
        for issue in parsed {
            if !seen.contains(&issue.number) {
                seen.push(issue.number);
                issues.push(issue);
            }
        }
    }

    issues.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    issues.truncate(limit);
    issues
}

/// The open pull requests in `scope` — a review requested from me or opened
/// by me, or every one — newest first, with the login `gh` is
/// authenticated as.
///
/// Not cached: only Start work's `pr-review` list reads it, once per open,
/// and a review request that arrived a minute ago should be in that list.
pub async fn list_pull_requests(
    path: &str,
    scope: Scope,
) -> Result<PullRequests, PullRequestsError> {
    let (auth, viewer) = get_auth_status().await;
    if let Some(error) = PullRequestsError::from_auth(auth) {
        // A killed probe reads as signed out but is never cached: `gh` did
        // not say so, and telling a signed-in user to log in would be false.
        return Err(if auth_answered() {
            error
        } else {
            PullRequestsError::Failed
        });
    }
    const JSON_FIELDS: &str = "number,title,author,isDraft,headRefName,isCrossRepository,createdAt";
    let mut answers = Vec::new();
    for filter in pull_request_filters(scope) {
        let mut args = vec!["pr", "list"];
        args.extend_from_slice(filter);
        args.extend_from_slice(&["--state", "open", "--json", JSON_FIELDS]);
        let (code, stdout, _) = run_gh(&args, Some(Path::new(path))).await;
        answers.push((code == 0).then_some(stdout));
    }
    let answers: Vec<Option<&str>> = answers.iter().map(Option::as_deref).collect();
    merge_pull_requests(&answers, viewer)
}

/// Whether `gh` gave an authentication answer, which is cached once given.
fn auth_answered() -> bool {
    state().lock().is_ok_and(|guard| guard.auth.is_some())
}

/// `pr-review`'s list as `gh` answered it.
#[derive(Debug, Clone, Default)]
pub struct PullRequests {
    pub prs: Vec<GitHubPullRequest>,
    /// The login `gh` is authenticated as.
    pub viewer: Option<String>,
    /// One of the queries the scope is made of failed: the PRs are the
    /// ones the others returned, and there may be more.
    pub incomplete: bool,
}

/// Why `pr-review` has no list at all. Kept apart from an empty list, so
/// a failure never reads as "you have no PRs".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestsError {
    NotInstalled,
    NotAuthenticated,
    /// Every query failed: a network error, a rate limit, a killed `gh`.
    Failed,
}

impl PullRequestsError {
    /// The error a `gh` in this state gives, if it cannot be asked at all.
    pub fn from_auth(auth: AuthStatus) -> Option<Self> {
        match auth {
            AuthStatus::Authenticated => None,
            AuthStatus::NotAuthenticated => Some(Self::NotAuthenticated),
            AuthStatus::NotInstalled => Some(Self::NotInstalled),
        }
    }

    /// What the list says in place of its PRs.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotInstalled => "gh is not installed",
            Self::NotAuthenticated => "gh is not logged in",
            Self::Failed => "Could not load the PRs from GitHub",
        }
    }
}

/// The PRs of several `gh pr list` answers, each listed once, newest first.
/// `None` is a query that exited non-zero; an answer that does not parse
/// failed as surely. Some failing marks the list incomplete; all failing
/// is an error. `viewer` is carried onto the list.
fn merge_pull_requests(
    answers: &[Option<&str>],
    viewer: Option<String>,
) -> Result<PullRequests, PullRequestsError> {
    let mut prs: Vec<GitHubPullRequest> = Vec::new();
    let mut failed = 0;
    for answer in answers {
        let parsed =
            answer.and_then(|output| serde_json::from_str::<Vec<GitHubPullRequest>>(output).ok());
        let Some(parsed) = parsed else {
            failed += 1;
            continue;
        };
        for pr in parsed {
            if !prs.iter().any(|seen| seen.number == pr.number) {
                prs.push(pr);
            }
        }
    }
    if failed == answers.len() {
        return Err(PullRequestsError::Failed);
    }
    prs.sort_by_key(|pr| std::cmp::Reverse(pr.created_at));
    Ok(PullRequests {
        prs,
        viewer,
        incomplete: failed > 0,
    })
}

/// Check out PR `number` in the worktree at `worktree`, on the PR's own
/// branch. `gh` knows where the head lives, including on a fork, which a
/// plain `git fetch` of the head branch would not.
pub async fn checkout_pull_request(worktree: &str, number: i64) -> Result<(), String> {
    let number = number.to_string();
    let (code, _, stderr) = run_gh(&["pr", "checkout", &number], Some(Path::new(worktree))).await;
    if code == 0 {
        Ok(())
    } else if stderr.is_empty() {
        Err(format!("gh pr checkout {number} failed"))
    } else {
        Err(stderr)
    }
}

/// The state of issue `number`, or `None` when `gh` could not say.
pub async fn issue_state(path: &str, number: i64) -> Option<ItemState> {
    item_state(path, "issue", number).await
}

/// The state of pull request `number`, or `None` when `gh` could not say.
pub async fn pull_request_state(path: &str, number: i64) -> Option<ItemState> {
    item_state(path, "pr", number).await
}

/// Not cached: the finished check that asks runs on the worktree sweep, and
/// the answer it wants is whether something changed since the last one.
async fn item_state(path: &str, kind: &str, number: i64) -> Option<ItemState> {
    let number = number.to_string();
    let (code, stdout, _) = run_gh(
        &[kind, "view", &number, "--json", "state"],
        Some(Path::new(path)),
    )
    .await;
    if code != 0 {
        return None;
    }
    parse_item_state(&stdout)
}

/// The `state` of a `gh … view --json state` answer. Anything else is
/// unknown rather than open, so a garbled answer never unmarks a finished
/// worktree.
fn parse_item_state(json: &str) -> Option<ItemState> {
    #[derive(serde::Deserialize)]
    struct Answer {
        state: ItemState,
    }
    serde_json::from_str::<Answer>(json)
        .ok()
        .map(|answer| answer.state)
}

/// Drop the cached issues for one repository path; see
/// [`IssueCache::invalidate`].
pub fn invalidate_cache(path: &str) {
    if let Ok(mut guard) = state().lock() {
        guard.issues.invalidate(path);
    }
}

impl Clone for Cached {
    fn clone(&self) -> Self {
        Self {
            issues: self.issues.clone(),
            fetched_at: self.fetched_at,
        }
    }
}

#[cfg(test)]
mod tests;
