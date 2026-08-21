# Repository settings this repo depends on

Settings are not in the tree, so they drift silently and nothing reviews them.
This file is the record of what SESH expects and why. It mirrors
`Model-Experiments/.github/SETUP.md`, which was built and proven first — the
point is that three repos share one design rather than inventing three.

## Diagnosing protection

Use **`GET /repos/{owner}/{repo}/rulesets`**, never
`GET /repos/{owner}/{repo}/branches/{branch}/protection`.

The second returns **404 for a ruleset-protected branch**, so a 404 means "no
*classic* protection", not "unprotected". That inference has already produced a
wrong conclusion twice, including a claim that all three repos were unprotected
when one was fully protected.

At the time of writing SESH had **zero rulesets** — genuinely unprotected,
confirmed by the correct probe.

## `delete_branch_on_merge`

Must stay **on**. It is what makes GitHub retarget a child PR when its parent
merges. Necessary and *not* sufficient: it does nothing about the squash
collision, which is what the ancestry audit catches.

## The default-branch ruleset

Target `refs/heads/~DEFAULT_BRANCH`, enforcement `active`, **`bypass_actors: []`**
— rulesets have no `enforce_admins`; an empty bypass list is the equivalent and
the strictest setting.

Rules:

| Rule | Why |
|---|---|
| `pull_request`, `required_approving_review_count: 0` | **Keep at 0.** A token cannot approve a PR and an author cannot self-approve, so raising it strands every auto-merge PR permanently and silently — the same failure class this whole effort is about. |
| `required_status_checks`, strict, context **`gate`** | One name, and one that cannot be filtered away. |
| `required_linear_history` | Keeps `master` readable. |
| `non_fast_forward` | No force-pushes. |
| `deletion` | The default branch cannot be deleted. |

### Why the required check is `gate` and not the real jobs

`.github/workflows/gate.yml` runs two jobs, `rust` and `surfaces`. Requiring
either **directly** works today and breaks the moment one gains a path filter: a
docs-only PR skips the job, the required check never reports, and the PR waits
forever with nothing to read. That is Parallax #43.

So the required context is the aggregator job `gate` — `if: always()`, `needs`
both halves, fails only on a real failure. A skipped dependency is not a
failure; it is a job that correctly had nothing to do.

**Never require a filterable job.**

## The tags ruleset

Target `tag`, restricting `deletion` and `non_fast_forward`, so a release tag
cannot be moved or removed after the fact.

## Branch hygiene

`.github/workflows/branch-hygiene.yml` and `.github/scripts/branch_hygiene.sh`
are copied **verbatim** from `Model-Experiments@main` — verified by matching git
blob SHAs, not by eye. Do not edit them here; fix them there and re-copy, or the
three repos diverge and the shared design stops being shared.

Two things that must stay true:

- the script is committed **`100755`**. It is invoked directly, and `100644`
  fails on the runner with "Permission denied".
- the audit job keeps **`fetch-depth: 0`**. `git merge-base` needs real history;
  a shallow clone reports every merge as stranded.

**Neither hygiene job may become a required status check.** `audit` reports on
repository state rather than on the diff, so a finding about one PR must never
put a red X on an unrelated one. `base-is-default` is diff-scoped and could
reasonably become required later.

## The allowlist

`.github/stale-base-allowlist.txt`, keyed `#<number>`. An entry goes in **only**
after confirming on `master` that the work actually shipped. "The PR says
merged" is not evidence — that is precisely the thing the guard exists to
disbelieve.
