# Notes for agents

## Read the spec for what you touch

[`spec/README.md`](spec/README.md) maps every crate, app, package and contract to the area docs for it. Before you change a part of Dassie, read [`spec/overview.md`](spec/overview.md), [`spec/engineering.md`](spec/engineering.md) and the area docs the index names for the paths you touch. They hold what the code cannot say: what a part is for, its invariants and contracts, what must never change, and why. The code is the only source for values, layouts and lists.

Keep the spec current in the same PR as the change, and keep it from becoming a second source of truth (the rules are in [`spec/README.md`](spec/README.md#what-belongs-here)):

- **A change to what a part guarantees, or why:** update its area doc. A changed value or list does not touch the spec. No history: no dates, milestones or PR numbers.
- **A choice between real alternatives:** add a decision in [`spec/decisions/`](spec/decisions/README.md), which keeps the reasons.
- **Work not built yet:** an open area in [`spec/roadmap.md`](spec/roadmap.md), never a numbered milestone.
- **A new crate, app, package or contract:** a row in the index.

Cite the spec by path and heading, as `spec/areas/peer-protocol.md#replay-protection`, or a decision by number, as `D23`. `node --test scripts/spec_test.ts` checks every link and citation, and the index.

## Keep the architecture page current

[dassie.land/architecture/](https://dassie.land/architecture/) shows how Dassie is built: its crates, apps, packages and contracts, how they connect, journeys through them, how they are tested, and a short changelog per release. The page draws everything from [`apps/website/src/architecture/map.json`](apps/website/src/architecture/map.json). It describes what is on `main`, not how it got there, so it carries no history beyond the releases.

Update `map.json` in the same PR as the change it describes:

- **A crate, app, package or contract is added, removed or renamed:** add, remove or rename its box in `components` (with its `path`, a short `label`, its subsystem `sub`, a position in a layer, `desc` and `does`), and its `edges`. Keep the boxes apart and inside their layer; to make room, move others.
- **What a part does, or what it builds on, changes:** update its `desc`, `does` and `edges`, and any journey in `flows` that passes through it.
- **A release is tagged:** add an entry at the top of `releases`, `{version, date, changes}`, with a few sentences on what people get, in plain words. Wrap commands and paths in backticks.

Write for someone new to Dassie, in the present tense, and leave out milestones, PR numbers and counts that go stale. `map_test.ts` fails when a crate, app, package or contract has no box, when a path or id is wrong, when boxes overlap or leave their layer, and when the newest release is not the version in `crates/dassie-cli/Cargo.toml`. Run it with `node --test apps/website/src/architecture/map_test.ts`.

## Workflow

All changes go through pull requests. Commit, push, and let CI validate. Never push to `main`.

Open a PR as a draft (`gh pr create --draft`) while you are still working on it, and mark it ready for review (`gh pr ready`) as soon as it is complete. A PR is a draft only while an agent is still editing it. Waiting for CI is not a reason to keep it a draft, because everyone can see the checks. A PR that reviewers have approved is ready to merge.

### Review

Edna reviews PRs. She is a Claude Code session on hecate named `edna-<project>`, that reviews as `vorstand-edna-bot`. Work tells her when a PR of an agent on hecate is ready, and tells you when her review is on GitHub; don't message her about a review:

1. Marking your PR ready (`gh pr ready`, or opening it ready) asks for her review. Put your reasons and trade-offs in the PR description, which is what the review reads, and for a stack its order (bottom first) in each PR's. Don't wait for its checks first: she reviews the code, not CI, and watching the checks and fixing what fails is yours.
2. Work tells you once her review is on GitHub (read it with `gh pr view <N> --comments`). If she requested changes, address each blocking finding: with a change, or, if you judge none is needed, with your reason in the PR description. Then request her review again: `gh pr edit <N> --remove-reviewer vorstand-edna-bot`, then `gh pr edit <N> --add-reviewer vorstand-edna-bot`. A push alone doesn't tell her. Repeat until she approves. Review is there to catch oversights; the trade-offs are yours.
3. If `edna-<project>` isn't in `ListAgents`, say so in your reply: she catches up when she starts again, and until then the PR waits.

Everyone else's PRs wait for a code owner as before.

### Merging

GitHub merges your PR. Once it is ready, turn on auto-merge (`gh pr merge <PR> --auto --merge`): it merges as soon as it is approved and its checks pass. (A stack of your own is the exception, below.)

- When Edna approves, look once at `gh pr view <PR> --json mergeable` (`UNKNOWN` means GitHub hasn't worked it out yet: look again in a minute). `CONFLICTING` never merges: rebase onto `origin/main`, push, request Edna's review again (as in "Review"), and turn auto-merge on again if GitHub turned it off.
- You are done when the PR has merged. Until then its checks are yours to watch: if one fails, fix it, push, and request Edna's review again.
- A push after approval needs approving again, so request Edna's review again: auto-merge never merges what she hasn't seen.
- PRs merge without being brought up to date with `main`, so two that passed alone meet first on `main`. If your PR's check fails in code you didn't touch, look at the same check on the latest commit of `main`: if it failed there too, fix it forward in a PR of its own (unless one is open already); once that has merged, rebase yours onto `origin/main`, push, and request Edna's review again.
- A stack of your own dependent PRs doesn't auto-merge: once all of them are approved and green, merge it with `gh stack merge <top PR> --yes --merge`.
