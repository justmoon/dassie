# Decisions are named, not numbered

**Areas:** [Engineering](../engineering.md)

## Facts

Several pull requests are open at once, and many of them add a decision. A number taken from a shared counter is free only until another pull request takes it first: the one that merges later renames its file and every citation it added, in code as in the spec, then waits on its checks and its review again, and may lose the next number the same way. A decision is cited in code comments, error messages and the RPC contract as well as in the spec, so a renumbering touches all of them. And a table to which every decision adds a row at its end conflicts with every other pull request that does the same.

## Decision

A decision's file name is its name: a few words that say what was decided (`own-fulfills.md`), chosen by its author. It is cited by its file, as `spec/decisions/own-fulfills.md` in code and as a relative link inside the spec, like the area docs. The index lists the decisions in the alphabetical order of their names. `scripts/spec_test.ts` checks that every citation names a decision that exists, that the index lists each decision once and in order, and that nothing cites a decision by a number.

## Accepted

- A citation is longer than a number, and wraps comments sooner.
- The numbers that decisions had before are translated only by [a table](README.md#former-numbers), for the commits, pull requests and issues that cite them. Their links to the numbered files no longer resolve on `main`.
- Two pull requests that pick the same name still conflict, but only when both decide about the same thing, and the conflict is then worth seeing.

## Rejected

- *A pull request's number as the decision's:* unique, but known only once the pull request is open, one pull request with two decisions needs more than its number, a decision's number would read as a pull request's, and the index would still conflict.
- *A number given when the pull request merges:* a change after review must be reviewed again, so the merge would never be the reviewed change.
- *A date or a hash:* unique, and meaningless in a comment.
- *Names for new decisions only:* two ways to cite a decision for good, and numbers could not be caught by a check.
