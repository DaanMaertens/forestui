# Reading sub-issues, dependencies and label sets through `gh`

Research for [#93](https://github.com/DaanMaertens/forestui/issues/93), part of
the coordinations map [#91](https://github.com/DaanMaertens/forestui/issues/91).
Question: which GitHub APIs give a coordination its work items and their order,
and what does `services/github.rs` already have that it can reuse.

Checked 2026-10-13 against github.com with `gh` 2.97.0. "Observed" marks a
claim confirmed by a read-only call against `DaanMaertens/forestui` (#91 has
sub-issues #92-#100, with blocked-by links between them) rather than by the docs.

## Short answer

- **`gh` already does most of it, with no `gh api` needed for one level.**
  `gh issue view N --json subIssues,subIssuesSummary,parent,blockedBy,blocking`
  returns the direct sub-issues (with number, state, title, url) and both
  dependency directions in a single GraphQL call. `gh issue list --json`
  takes the same fields. gh asks for `subIssues(first:100)` and
  `blockedBy/blocking(first:50)`, which equal GitHub's own caps, so nothing is
  truncated (observed via `GH_DEBUG=api`).
- **A whole tree with dependencies takes one `gh api graphql` call per two
  levels, not one call for the whole tree.** Nesting goes up to eight levels,
  and the 500,000-node limit rejects three levels of `subIssues(first:100)`.
  Two levels plus dependencies fit, and cost 52 points (observed).
- **Order is the parent's own order.** Sub-issues come back in the parent's list
  order, which users change by dragging (REST "reprioritize"). There is no sort
  argument and no state filter, so closed sub-issues are always included and
  are filtered client-side.
- **Label queues are a search.** `gh issue list --label X` runs a GraphQL
  `search` (type `ISSUE_ADVANCED`), not the repository issues connection, so
  `blockedBy` comes along for free and several `--label` flags are AND.
- **Absence shows up as a refused field, not an empty list.** An old `gh` says
  `Unknown JSON field`; an old server says the field does not exist; a repo with
  issues off gives 410. forestui already handles the first one for `blockedBy`.

## Sub-issues

### Limits and availability

- Up to **100 sub-issues per parent** and **eight levels of nesting**
  ([Adding sub-issues](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues)).
- A sub-issue can come from another repository, but "must belong to the same
  repository owner as the parent issue"
  ([REST sub-issues, Add sub-issue](https://docs.github.com/en/rest/issues/sub-issues#add-sub-issue)).
  A coordination's items can therefore live in sibling repos of one owner, and
  each node has to carry its `repository { nameWithOwner }`. forestui maps
  issues to checkouts by path today, so this is a real case to decide.
- Generally available since April 2025
  ([changelog](https://github.blog/changelog/2025-04-09-evolving-github-issues-and-projects/)).
  The docs name no plan restriction. No GitHub Enterprise Server docs page for
  the sub-issues REST endpoints could be found (3.15-3.19 all 404), so do not
  assume GHES support.

### `gh` (the path to use)

```sh
gh issue view 91 --json subIssues,subIssuesSummary,parent
```

Observed shape:

```json
{"subIssues":{"nodes":[{"id":"I_kw…","number":92,"state":"OPEN","title":"…","url":"…"}, …],"totalCount":9},
 "subIssuesSummary":{"completed":0,"percentCompleted":0,"total":9},
 "parent":null}
```

- One level only. `nodes` has no `labels`, `blockedBy` or sub-issues of its own;
  for those, run a second `gh issue list` or use GraphQL (below).
- The node has `number` and `url`, but no repository field. For a cross-repo
  sub-issue the repo has to be parsed from `url`.
- `parent` is `{id, number, state, title, url}` or `null`.

### GraphQL

From introspection of the `Issue` type (observed):

| field | arguments | notes |
|---|---|---|
| `subIssues` | `first/last/after/before` | "A list of sub-issues associated with the Issue." No `states`, no `orderBy`. |
| `subIssuesSummary` | none | `total`, `completed`, `percentCompleted` |
| `parent` | none | "The parent entity of the issue." |
| `trackedIssues` / `trackedInIssues` | paging | the older tasklist relation, **not** sub-issues; ignore |

The mutations are `addSubIssue`, `removeSubIssue` and `reprioritizeSubIssue`.

### REST

([REST sub-issues](https://docs.github.com/en/rest/issues/sub-issues))

- `GET /repos/{o}/{r}/issues/{n}/sub_issues`: an array of full issue objects,
  `per_page` up to 100 (default 30), `page`. Codes are 200/404/410.
- `GET /repos/{o}/{r}/issues/{n}/parent`: the parent issue. With no parent it
  returns 404 `"No parent issue found"` (observed), so "no parent" and "not found"
  share a status code.
- Every REST issue object carries `parent_issue_url`, `sub_issues_summary`
  `{total, completed, percent_completed}` and `issue_dependencies_summary`
  (observed).
- Writes identify issues by the database `id` (an integer), not by `number`.

### Order

- `PATCH …/sub_issues/priority` with `after_id` or `before_id` moves "a
  sub-issue to a different position in the parent list". The list is therefore
  ordered, and that order is user-controlled.
- Neither REST nor GraphQL offers a sort argument. Both returned #92-#100 in the
  same order (observed). Nothing in this repo had been reordered, so "the API
  returns UI order" is inferred, not proven. A test with a single reprioritize
  call would settle it, but that is a write and was left out here.

## Dependencies (blocked by / blocking)

### Limits and availability

- **50 issues per relationship type** ("blocked by" and "blocking"). Generally
  available since 2025-08-21, "fully supported in the API and webhooks"
  ([changelog](https://github.blog/changelog/2025-08-21-dependencies-on-issues/)).
- The REST docs exist for GHES 3.19
  ([GHES 3.19](https://docs.github.com/en/enterprise-server@3.19/rest/issues/issue-dependencies))
  but not for 3.18. That suggests GHES has it from 3.19 on, though a missing
  docs page is weak evidence.
- GitHub draws no ordering from dependencies: a blocked issue only gets a
  "Blocked" icon
  ([Creating issue dependencies](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-issue-dependencies)).
  "Run blockers first" is up to the coordination to enforce.
- A blocker can be in another repository. Carry `repository { nameWithOwner }`,
  or parse `url`.

### `gh`

```sh
gh issue view 95 --json blockedBy,blocking
# {"blockedBy":{"nodes":[{"id":"…","number":93,"state":"OPEN","title":"…","url":"…"}],"totalCount":1},
#  "blocking":{"nodes":[],"totalCount":0}}
```

Both are also accepted by `gh issue list --json` (forestui already uses
`blockedBy` there). gh fetches `first:50`, the cap.

### GraphQL

| field | arguments |
|---|---|
| `blockedBy` | paging, `orderBy: IssueDependencyOrder` with `field` `DEPENDENCY_ADDED_AT` or `CREATED_AT` |
| `blocking` | same |
| `issueDependenciesSummary` | none. `blockedBy`/`blocking` count **open** ones; `totalBlockedBy`/`totalBlocking` count "open and closed" (field descriptions) |

The mutations are `addBlockedBy` and `removeBlockedBy`; there is no separate
"add blocking" mutation, because a blocking link is the same edge seen from
the other end.

`issueDependenciesSummary.blockedBy == 0` is the cheap "ready to start" test.
It needs no node list, so it is the field to request when only readiness
matters.

### REST

([REST issue dependencies](https://docs.github.com/en/rest/issues/issue-dependencies))

- `GET …/issues/{n}/dependencies/blocked_by` and `GET …/dependencies/blocking`:
  arrays of full issues, `per_page` ≤ 100, `page`. Codes are 200/301/404/410.
- `POST …/dependencies/blocked_by` with `{issue_id}`, and
  `DELETE …/dependencies/blocked_by/{issue_id}`.

## One query for a whole tree with dependencies

GraphQL can nest it, but the node limit limits the depth:

- The limits are 1-100 per connection, **500,000 nodes per call**, and a cost
  of roughly (sum of connection requests) / 100, with 5,000 points/hour per user
  and 2,000 points/minute secondary
  ([GraphQL rate and query limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)).
- Three levels of `subIssues(first:100)` fail before running with
  `MAX_NODE_LIMIT_EXCEEDED` ("requesting up to 1,000,000 possible nodes")
  (observed).
- Two levels at `first:100` / `first:50`, each with `blockedBy(first:50)`,
  run; **cost 52** (observed). One level with `blockedBy`, `blocking`, labels
  and one nested `subIssues(first:50)` cost 27 (observed).

A query that works on #91:

```graphql
query($owner:String!, $repo:String!, $n:Int!) {
  repository(owner:$owner, name:$repo) {
    issue(number:$n) {
      number state
      subIssuesSummary { total completed }
      subIssues(first:100) {
        totalCount
        nodes {
          id number title state stateReason url
          repository { nameWithOwner }
          labels(first:20) { nodes { name } }
          issueDependenciesSummary { blockedBy totalBlockedBy }
          blockedBy(first:50) { nodes { number state repository { nameWithOwner } } }
          subIssuesSummary { total }
        }
      }
    }
  }
}
```

For deeper trees: fetch the level, collect the node ids whose
`subIssuesSummary.total > 0`, then fetch their children in one call with
`nodes(ids:[…]) { ... on Issue { subIssues(first:100) { … } } }`. That is one call
per level and at most eight. Most trees are one or two levels deep, so the
first call usually answers everything.

## Open issues by label

- `gh issue list --label L [--label M] --state open --json …` (default
  `--limit 30`). Under `GH_DEBUG=api` it sends a GraphQL `search` with query
  `label:L repo:o/r state:open type:issue` and `type: ISSUE_ADVANCED` (observed).
  The consequences:
  - Several `--label` flags are AND. For OR, use `--search 'label:"a","b"'`. The
    advanced syntax also takes `AND`/`OR` and parentheses five levels deep
    ([Filtering and searching](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/filtering-and-searching-issues-and-pull-requests)).
  - Search caps at 1,000 results
    ([REST search](https://docs.github.com/en/rest/search/search)). That is
    irrelevant for a queue.
  - Search sorts by relevance unless `sort:created-asc` (or similar) is in
    `--search`. Pin it, because a label queue has no order of its own.
- Search qualifiers that work against this repo (observed): `is:blocked`,
  `is:blocking`, `has:parent-issue`, `no:parent-issue`. So
  `gh issue list --search 'label:L -is:blocked sort:created-asc'` is the
  "ready" subset in one call. `parent-issue:owner/repo#91` and
  `parent-issue:91` returned nothing, and `has:sub-issues` matched issues with
  no sub-issues. Do not rely on those three; read sub-issues from the parent
  instead.
- Search through GraphQL counts against the GraphQL points budget. The REST
  `/search/issues` endpoint has its own 30 requests/minute limit; going through
  `gh issue list` avoids it.

## Pagination and rate limits, summed up

| route | page size | cap that matters |
|---|---|---|
| `gh issue view --json subIssues` | gh takes 100 | 100 per parent, so never paged |
| `gh … --json blockedBy/blocking` | gh takes 50 | 50 per type, so never paged |
| REST list endpoints | 30 default, 100 max | `gh api --paginate` follows `Link` |
| GraphQL connections | 1-100 | 500k nodes/call, 5,000 pts/h, 2,000 pts/min |
| `gh issue list` (search) | `--limit` | 1,000 results |

A poll every 30 s of a two-level tree at 52 points uses about 6,240 points/hour,
which is **more than the 5,000/hour budget**, and the budget is shared with
everything else the user's `gh` does. One level at around 1-5 points is fine,
or poll on the coordination's own events (a worker finished) rather than on a
clock. The REST core budget is a separate 5,000/hour (`X-RateLimit-Resource:
core`, observed).

## Detecting absence

| situation | what you see | how to treat it |
|---|---|---|
| `gh` older than the fields | `Unknown JSON field: "subIssues"`, non-zero exit | the same retry forestui has for `blockedBy` (`rejects_blocked_by`); with no sub-issues there is no tree queue |
| server without the field (older GHES) | GraphQL error "Field '…' doesn't exist on type 'Issue'" | treat as unsupported, not as empty |
| repo with issues disabled | REST 410 Gone; GraphQL `hasIssuesEnabled: false` | no queue |
| issue has no parent | REST `…/parent` 404 "No parent issue found"; GraphQL `parent: null` | use GraphQL or `gh … --json parent` to avoid the ambiguous 404 |
| feature present, nothing linked | `totalCount: 0`, summary `total: 0` | genuinely empty |

## What forestui already has

`src/services/github.rs`, with `src/models.rs` beside it:

- **`run_gh`**: a subprocess wrapper that returns `(code, stdout, stderr)`,
  with `-1` for a missing binary and `KILLED` (`-2`) for a process killed by a
  signal. A coordination should call through it too, so a killed probe is
  never read as "no items".
- **`get_auth_status`**: auth cached for the process, with a killed probe
  never cached. It is the gate before any fetch.
- **`get_repo_info(path)`** returns `(owner, repo)` via `gh repo view`, and
  `app/repo_settings.rs` learns and caches the `owner/name` key per checkout in
  the background. A GraphQL query needs owner/name, so use that.
- **`ISSUE_FIELDS` already includes `blockedBy`**, together with
  `rejects_blocked_by` and the `LEGACY_ISSUE_FIELDS` retry for an old `gh`.
  `GitHubIssue::{open_blockers, is_blocked}` in `models.rs` already define
  "blocked" as at least one open blocker. A coordination's readiness rule
  should reuse that rule as it stands, not restate it.
- **`IssueCache`**: the issue list keyed by `(path, Scope)`, a 300 s TTL,
  generation-fenced invalidation, and failures stored as already-stale. A label
  queue is a third "scope" of this list (it takes `--label` instead of
  `--assignee/--author`), so it could live in the same cache under a new key.
  A sub-issue tree is a different question and wants its own entry, keyed by
  the parent.
- **`issue_state` / `item_state`**: an uncached `gh issue view N --json state`,
  which the finished check uses on each worktree sweep. To advance a queue, the
  same call could add `stateReason`, so that "closed as not planned" is told
  apart from "completed".
- **Missing:** no call reads `subIssues`, `parent` or `blocking`, and there is
  no `gh api graphql` anywhere yet. One level of sub-issues can be added as a
  `gh issue view --json subIssues,subIssuesSummary`. A second field list then
  needs the same old-`gh` retry. A multi-level tree needs the first
  `gh api graphql` call.

## Surprises

- `gh` 2.97 has first-class flags for all of this, both to read
  (`--json subIssues,parent,blockedBy,blocking,subIssuesSummary`) and to write
  (`gh issue edit --add-sub-issue/--parent/--add-blocked-by/--add-blocking`,
  `gh issue create --parent/--blocked-by/--blocking`).
- `gh issue list` runs every query through search, so `--label` filters inherit
  search semantics, including relevance order unless a sort is given.
- `has:sub-issues` silently matched issues with no sub-issues, and
  `parent-issue:` matched nothing.
- Polling a two-level tree every 30 s costs more than the hourly GraphQL budget.
