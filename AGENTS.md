# Notes for agents

## Keep the progress map current

The [Dassie Rewrite Map](https://claude.ai/artifact/Jiuea7GeGTv2vgY6xsLyJH) shows the milestones of [spec 7 §8](spec/07-rewrite.md#8-milestones) and the components they built. Its page draws everything from its artifact database and redraws for open viewers as soon as the data changes. So never republish the page to update it. Write the database with the `ArtifactData` tool instead.

When you open a milestone's PR, and again once it merges:

1. Read `map/state`. It is one document: `milestones` keyed by milestone (`status` is `planned`, `wip` or `done`, and `touched` lists component ids), `components` keyed by id (`st` uses the same three values, plus position, size and text), `edges`, `facts`, `lede*`, `asOf` and `source`.
2. Change what the milestone changed. A milestone that starts gets its own colour (`c`: `--m6` to `--m9`), and its components move from `planned` to `wip`. New crates or apps get a box and their edges. A merge sets `done`, `when` (for example "Merged Oct 20 · PR #42") and `short`, and refreshes `facts`, `asOf`, `source` and the line and test counts of what changed. Then write it back with `if_version`.
3. Add `updates/<yyyy-mm-dd>-<milestone>` with `{at, milestone, kind: "started" | "merged", text}`, where `text` is one sentence.

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
