# Notes for agents

## Keep the progress map current

The [Dassie Rewrite Map](https://claude.ai/artifact/Jiuea7GeGTv2vgY6xsLyJH) shows the milestones of [spec 7 §8](spec/07-rewrite.md#8-milestones) and the components they built. Its page draws everything from its artifact database and redraws for open viewers as soon as the data changes. So never republish the page to update it. Write the database with the `ArtifactData` tool instead.

When you open a milestone's PR, and again once it merges:

1. Read `map/state`. It is one document: `milestones` keyed by milestone (`status` is `planned`, `wip` or `done`, and `touched` lists component ids), `components` keyed by id (`st` uses the same three values, plus position, size and text), `edges`, `facts`, `lede*`, `asOf` and `source`.
2. Change what the milestone changed. A milestone that starts gets its own colour (`c`: `--m6` to `--m9`), and its components move from `planned` to `wip`. New crates or apps get a box and their edges. A merge sets `done`, `when` (for example "Merged Oct 20 · PR #42") and `short`, and refreshes `facts`, `asOf`, `source` and the line and test counts of what changed. Then write it back with `if_version`.
3. Add `updates/<yyyy-mm-dd>-<milestone>` with `{at, milestone, kind: "started" | "merged", text}`, where `text` is one sentence.
