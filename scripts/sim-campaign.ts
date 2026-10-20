// The simulation campaign's supervisor (spec D27): one loop of it, on a
// campaign VM.
//
//   node scripts/sim-campaign.ts [--minutes N] [--dry-run [--unsandboxed]]
//
// A loop picks what to run: `main`, the release candidate (the `release`
// branch, once it is on `main`), or an open pull request labelled `sim`.
// It builds that commit's `dassie-sim` and runs `dassie-sim campaign` on
// every core for an hour, then reports: a trusted commit (main, the release
// candidate) gets a `sim-campaign` commit status and one issue per failure
// signature; a pull request gets comments on itself, and nothing else.
//
// The loop then exits, and systemd starts the next one from a fresh
// checkout of `main` (scripts/sim-campaign/dassie-sim-campaign.service), so
// a change to this script, or to the runner, reaches every VM within a loop.
//
// Only this process holds the GitHub credentials, in memory. Every build
// and run happens in a systemd sandbox as a user of its own, with no
// network and nothing of the supervisor's to read, and what it prints is
// untrusted data (spec/areas/testing.md#the-campaign).
//
// It reads its configuration from the environment, which the VM's startup
// script writes (scripts/sim-campaign/startup.sh): SIM_CAMPAIGN_SECRET,
// the Secret Manager version holding the GitHub App's private key (or a
// token, without SIM_CAMPAIGN_APP_ID), SIM_CAMPAIGN_APP_ID, and
// SIM_CAMPAIGN_DIR, where it keeps its work. `--dry-run` writes nothing to
// GitHub and needs no credentials; `--unsandboxed` runs the steps directly,
// on a machine without the sandbox users, and only in a dry run.

import { spawn } from "node:child_process";
import { createSign } from "node:crypto";
import { mkdir, rm } from "node:fs/promises";
import { availableParallelism } from "node:os";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const REPO = "justmoon/dassie";

/** The status context the release check reads. */
export const STATUS_CONTEXT = "sim-campaign";

/** Runs a trusted commit needs before its status is `success`, with no
 * failure but the tripwire's. A failure that hits a share p of the seeds
 * shows within 3/p runs with 95% confidence (the rule of three), so 30,000
 * runs hold a release until a failure of one seed in 10,000 would most
 * likely have shown. The runs are spread over every simulation, so for any
 * one of them the bound is looser by their number. */
export const CLEAR_RUNS = 30_000;

/** The shares of loops, by what they run. The release candidate's share
 * goes to `main` while there is none, or once its status is settled; the
 * pull requests' share is split among them. */
export const SHARES = { main: 2, release: 1, pr: 1 };

/** The labels the campaign uses, created if missing. */
export const LABELS = {
  failure: {
    name: "sim-failure",
    color: "b60205",
    description: "A failure the simulation campaign found (D27)",
  },
  tripwire: {
    name: "sim-tripwire",
    color: "fbca04",
    description: "The campaign's tripwire, which fails on purpose (D27)",
  },
  optIn: {
    name: "sim",
    color: "1d76db",
    description: "Run this pull request in the simulation campaign (D27)",
  },
};

/** GitHub keeps at most 1,000 statuses per commit and context. A commit
 * stops getting new ones short of that, but for a failure. */
const STATUS_CAP = 990;
/** A status description is at most 140 characters. */
const DESCRIPTION_BYTES = 140;
/** Pages of 100 read from a list, at most. */
const PAGES_MAX = 10;
/** Distinct failure signatures one loop keeps; the rest are counted. */
const SIGNATURES_MAX = 64;
/** Commits a failure's state remembers it on. */
const COMMITS_MAX = 10;
/** Comments a failure's issue gets for new commits, at most. */
const ISSUE_COMMENTS_MAX = 20;
/** The longest line of runner output read; a longer one is invalid. */
const LINE_BYTES_MAX = 1 << 20;
/** How much of each text from a run an issue or comment shows. */
const TEXT_BYTES_MAX = { message: 2_000, options: 2_000, trace: 6_000 };
/** The end of a failed build's output a comment shows. */
const BUILD_LOG_BYTES_MAX = 6_000;
const SIGNATURE_BYTES_MAX = 200;

// Records

export type Outcome = "pass" | "safety" | "liveness" | "unsupported";
const OUTCOMES: Outcome[] = ["pass", "safety", "liveness", "unsupported"];

/** One finished run, as `dassie-sim campaign` prints it. */
export interface RunRecord {
  sim: string;
  /** Decimal, as printed: a u64 seed does not fit a JavaScript number. */
  seed: string;
  outcome: Outcome;
  signature: string;
  ms: number;
  message: string;
  options: string;
  traceTail: string;
}

export interface Summary {
  runs: number;
  seconds: number;
}

export type Line =
  | { kind: "run"; run: RunRecord }
  | { kind: "summary"; summary: Summary }
  | { kind: "invalid"; reason: string };

/** The signature as the campaign files it: printable, bounded, and safe
 * inside an HTML comment and a code span. */
export function cleanSignature(text: string): string {
  return text.replace(/[^A-Za-z0-9_.:/@+=-]/g, "_").slice(
    0,
    SIGNATURE_BYTES_MAX,
  );
}

const U64_MAX = (1n << 64n) - 1n;

/** A text field, cut to `max` characters: its start, or for a trace, its
 * end. */
function text(value: unknown, max: number, end = false): string {
  if (typeof value !== "string") return "";
  return end ? value.slice(-max) : value.slice(0, max);
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : undefined;
}

/** Parses one line of `dassie-sim campaign`'s output. The output of a pull
 * request's runner is untrusted, so anything unexpected is an invalid
 * line, never an exception. */
export function parseLine(line: string): Line {
  if (line.length > LINE_BYTES_MAX) {
    return { kind: "invalid", reason: "too long" };
  }
  let value: unknown;
  try {
    // A seed is a u64, which a number loses, so it is read from its source.
    value = (JSON.parse as (
      text: string,
      reviver: (
        key: string,
        value: unknown,
        context?: { source?: string },
      ) => unknown,
    ) => unknown)(
      line,
      (key, value, context) =>
        key === "seed" && typeof value === "number" && context?.source
          ? context.source
          : value,
    );
  } catch {
    return { kind: "invalid", reason: "not JSON" };
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return { kind: "invalid", reason: "not an object" };
  }
  const record = value as Record<string, unknown>;
  if ("summary" in record) {
    const summary = record.summary as Record<string, unknown> | null;
    const runs = count(summary?.runs);
    const seconds = typeof summary?.seconds === "number" &&
        Number.isFinite(summary.seconds) && summary.seconds >= 0
      ? summary.seconds
      : undefined;
    if (runs === undefined || seconds === undefined) {
      return { kind: "invalid", reason: "malformed summary" };
    }
    return { kind: "summary", summary: { runs, seconds } };
  }
  const { sim, seed, outcome, signature, ms } = record;
  if (typeof sim !== "string" || !/^[a-z][a-z0-9_-]{0,31}$/.test(sim)) {
    return { kind: "invalid", reason: "malformed sim" };
  }
  if (
    typeof seed !== "string" || !/^[0-9]{1,20}$/.test(seed) ||
    BigInt(seed) > U64_MAX
  ) {
    return { kind: "invalid", reason: "malformed seed" };
  }
  if (!OUTCOMES.includes(outcome as Outcome)) {
    return { kind: "invalid", reason: "malformed outcome" };
  }
  if (typeof signature !== "string" || count(ms) === undefined) {
    return { kind: "invalid", reason: "malformed signature or ms" };
  }
  return {
    kind: "run",
    run: {
      sim,
      seed: BigInt(seed).toString(),
      outcome: outcome as Outcome,
      signature: cleanSignature(signature),
      ms: ms as number,
      message: text(record.message, TEXT_BYTES_MAX.message),
      options: text(record.options, TEXT_BYTES_MAX.options),
      traceTail: text(record.trace_tail, TEXT_BYTES_MAX.trace, true),
    },
  };
}

/** The tripwire fails on purpose, so its failures show the campaign runs,
 * and hold up nothing. */
export function isTripwire(run: RunRecord): boolean {
  return run.sim === "tripwire";
}

export function isFailure(run: RunRecord): boolean {
  return run.outcome === "safety" || run.outcome === "liveness";
}

/** Every run of a signature in a loop, and the one that failed fastest,
 * whose seed replays quickest. */
export interface Failure {
  signature: string;
  tripwire: boolean;
  count: number;
  fastest: RunRecord;
}

/** What one loop's runs add up to. */
export class Tally {
  runs = 0;
  trips = 0;
  invalid = 0;
  /** Failures past SIGNATURES_MAX distinct signatures. */
  dropped = 0;
  failures = new Map<string, Failure>();
  summary: Summary | undefined;

  add(line: Line): void {
    if (line.kind === "invalid") {
      this.invalid += 1;
      return;
    }
    if (line.kind === "summary") {
      this.summary = line.summary;
      return;
    }
    const { run } = line;
    this.runs += 1;
    if (!isFailure(run)) return;
    if (isTripwire(run)) this.trips += 1;
    const known = this.failures.get(run.signature);
    if (known) {
      known.count += 1;
      if (run.ms < known.fastest.ms) known.fastest = run;
    } else if (this.failures.size < SIGNATURES_MAX) {
      this.failures.set(run.signature, {
        signature: run.signature,
        tripwire: isTripwire(run),
        count: 1,
        fastest: run,
      });
    } else {
      this.dropped += 1;
    }
  }

  /** The failures that hold up a commit: all but the tripwire's. */
  real(): Failure[] {
    return [...this.failures.values()].filter((failure) => !failure.tripwire);
  }
}

// The commit status

export type StatusState = "pending" | "success" | "failure" | "error";

export interface StatusEntry {
  state: StatusState;
  description: string;
  creator: string;
  targetUrl: string;
}

export interface NewStatus {
  state: "pending" | "success" | "failure";
  description: string;
  targetUrl?: string;
}

/** What the campaign has done on a commit, from every status it posted
 * there. The state lives only in the statuses, since spot VMs lose theirs. */
export interface Clearing {
  runs: number;
  seconds: number;
  trips: number;
  failed: boolean;
  /** The failure's link, from its status. */
  failureUrl: string;
  /** The campaign's own statuses on the commit. */
  statuses: number;
  latest: StatusState | undefined;
}

const INCREMENT = /\[\+([0-9]+) \+([0-9]+)s \+([0-9]+)t\]$/;

/** Folds the commit's statuses, newest first as GitHub lists them. Each
 * status records what its loop added, so that two VMs reporting at once
 * both count; a failure anywhere in the history is final. Only the
 * campaign's own statuses count: anyone with write access can post one in
 * its context. */
export function foldStatuses(entries: StatusEntry[], login: string): Clearing {
  const clearing: Clearing = {
    runs: 0,
    seconds: 0,
    trips: 0,
    failed: false,
    failureUrl: "",
    statuses: 0,
    latest: undefined,
  };
  for (const entry of entries) {
    if (entry.creator !== login) continue;
    clearing.statuses += 1;
    clearing.latest ??= entry.state;
    if (entry.state === "failure") {
      clearing.failed = true;
      clearing.failureUrl ||= entry.targetUrl;
    }
    const match = entry.description.match(INCREMENT);
    if (match) {
      clearing.runs += Number(match[1]);
      clearing.seconds += Number(match[2]);
      clearing.trips += Number(match[3]);
    }
  }
  return clearing;
}

/** Whether a commit's campaign is over: failed, or cleared. */
export function settled(clearing: Clearing): boolean {
  return clearing.failed || cleared(clearing);
}

/** A commit is cleared once it ran long enough with no failure, and the
 * tripwire tripped on it: a runner that stopped reporting failures would
 * otherwise clear every commit. */
function cleared(clearing: Clearing): boolean {
  return !clearing.failed && clearing.runs >= CLEAR_RUNS &&
    clearing.trips > 0;
}

export interface LoopCounts {
  runs: number;
  seconds: number;
  trips: number;
}

/** The status a loop posts: its own counts, after the totals so far. */
export function nextStatus(
  before: Clearing,
  loop: LoopCounts,
  failure?: { signature: string; url: string },
): NewStatus {
  assert(Number.isSafeInteger(loop.runs) && loop.runs >= 0, "loop runs");
  const seconds = Math.round(loop.seconds);
  const total: Clearing = {
    ...before,
    runs: before.runs + loop.runs,
    seconds: before.seconds + seconds,
    trips: before.trips + loop.trips,
    failed: before.failed || failure !== undefined,
  };
  const increment = ` [+${loop.runs} +${seconds}s +${loop.trips}t]`;
  const hours = (total.seconds / 3600).toFixed(1);
  const totals = `${total.runs} runs, ${hours} h, ${total.trips} trips`;
  let status: NewStatus;
  if (failure) {
    status = {
      state: "failure",
      description: `failed: ${failure.signature}; ${totals}`,
      targetUrl: failure.url,
    };
  } else if (total.failed) {
    status = {
      state: "failure",
      description: `failed earlier; ${totals}`,
      targetUrl: before.failureUrl || undefined,
    };
  } else if (cleared(total)) {
    status = { state: "success", description: `cleared: ${totals}` };
  } else {
    const waiting = total.runs < CLEAR_RUNS
      ? `of ${CLEAR_RUNS}`
      : "no trip yet";
    status = {
      state: "pending",
      description:
        `${total.runs} runs ${waiting}, ${hours} h, ${total.trips} trips`,
    };
  }
  // The counts are what later loops add up, so the text before them gives
  // way when the whole does not fit.
  const room = DESCRIPTION_BYTES - increment.length;
  status.description = status.description.slice(0, room) + increment;
  assert(status.description.length <= DESCRIPTION_BYTES, "description");
  return status;
}

// Issues and comments

/** What an issue or a pull request comment keeps about a signature, in a
 * hidden comment of its body, so that a later loop can add to it. */
export interface FailureState {
  signature: string;
  sim: string;
  outcome: Outcome;
  seen: number;
  /** The latest commits it was seen on, oldest first. */
  commits: string[];
  comments: number;
  fastest: {
    seed: string;
    ms: number;
    commit: string;
    message: string;
    options: string;
    traceTail: string;
  };
}

export function signatureMarker(signature: string): string {
  return `<!-- sim-campaign signature ${cleanSignature(signature)} -->`;
}

/** The state is a body's last line and the signature its first, where
 * no text from a run can put another: a run's texts sit in fenced blocks
 * between them. */
const STATE = /\r?\n<!-- sim-campaign state ([A-Za-z0-9+/=]+) -->\s*$/;

/** Whether `marker` is the first line of `body`. */
function firstLine(body: string, marker: string): boolean {
  return body.startsWith(marker) && /^\r?\n/.test(body.slice(marker.length));
}

/** Whether a body is the one for `signature`. */
export function isFor(body: string, signature: string): boolean {
  return firstLine(body, signatureMarker(signature));
}

function encodeState(state: FailureState): string {
  const data = Buffer.from(JSON.stringify(state)).toString("base64");
  return `<!-- sim-campaign state ${data} -->`;
}

/** The state in a body, if it has one that parses: people edit bodies. */
export function decodeState(body: string): FailureState | undefined {
  const match = body.match(STATE);
  if (!match) return undefined;
  try {
    const state = JSON.parse(Buffer.from(match[1], "base64").toString());
    if (
      typeof state?.signature !== "string" || count(state.seen) === undefined ||
      !Array.isArray(state.commits) || typeof state.fastest?.seed !== "string"
    ) {
      return undefined;
    }
    return state as FailureState;
  } catch {
    return undefined;
  }
}

/** Adds a loop's failures of one signature on `commit` to what was known,
 * and says whether the commit is new to it. */
export function mergeFailure(
  known: FailureState | undefined,
  failure: Failure,
  commit: string,
): { state: FailureState; newCommit: boolean } {
  const run = failure.fastest;
  const fastest = {
    seed: run.seed,
    ms: run.ms,
    commit,
    message: run.message,
    options: run.options,
    traceTail: run.traceTail,
  };
  if (!known) {
    return {
      state: {
        signature: failure.signature,
        sim: run.sim,
        outcome: run.outcome,
        seen: failure.count,
        commits: [commit],
        comments: 0,
        fastest,
      },
      newCommit: true,
    };
  }
  const newCommit = !known.commits.includes(commit);
  const commits = newCommit
    ? [...known.commits, commit].slice(-COMMITS_MAX)
    : known.commits;
  return {
    state: {
      ...known,
      seen: known.seen + failure.count,
      commits,
      fastest: run.ms < known.fastest.ms ? fastest : known.fastest,
    },
    newCommit,
  };
}

/** A fenced block that the text cannot close. */
export function fenced(text: string, language = ""): string {
  const longest = Math.max(
    0,
    ...[...text.matchAll(/`+/g)].map((match) => match[0].length),
  );
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}${language}\n${text}\n${fence}`;
}

export function reproduce(sim: string, seed: string, commit: string): string {
  return [
    `git checkout ${commit}`,
    `cargo run -p dassie-sim --profile sim -- run ${sim} ${seed}`,
    `cargo run -p dassie-sim --profile sim -- explain ${sim} ${seed}`,
  ].join("\n");
}

function details(state: FailureState): string {
  const { fastest } = state;
  const parts = [
    `The seed that failed fastest, ${fastest.seed} (in ${fastest.ms} ms), on ${fastest.commit}:`,
    fenced(reproduce(state.sim, fastest.seed, fastest.commit), "sh"),
  ];
  if (fastest.message) parts.push("What failed:", fenced(fastest.message));
  if (fastest.options) {
    parts.push("Its configuration:", fenced(fastest.options));
  }
  if (fastest.traceTail) {
    parts.push("The end of its trace:", fenced(fastest.traceTail));
  }
  return parts.join("\n\n");
}

export function issueTitle(signature: string): string {
  return `sim: ${cleanSignature(signature)}`;
}

export function issueBody(state: FailureState): string {
  const intro = state.sim === "tripwire"
    ? "The campaign's tripwire, which fails on purpose on a small share of seeds. While it keeps getting seeds, the campaign runs; if it stops, the campaign has stopped."
    : "The simulation campaign found this failure on a trusted commit.";
  return [
    signatureMarker(state.signature),
    `${intro} Its signature is \`${state.signature}\`.`,
    `Seen ${state.seen} times, latest on ${state.commits.at(-1)} (commits: ${
      state.commits.join(", ")
    }).`,
    details(state),
    "A seed reproduces a failure only at the commit it ran on, so a fix comes with a targeted test (D27).",
    encodeState(state),
  ].join("\n\n");
}

export function pullRequestComment(state: FailureState): string {
  return [
    signatureMarker(state.signature),
    `The simulation campaign found a failure on this pull request: \`${state.signature}\`, seen ${state.seen} times, latest on ${
      state.commits.at(-1)
    }.`,
    details(state),
    encodeState(state),
  ].join("\n\n");
}

const SUMMARY_MARKER = "<!-- sim-campaign summary -->";

export function pullRequestSummary(
  sha: string,
  outcome: { runs: number; seconds: number; failures: number } | {
    buildLog: string;
  },
): string {
  const text = "buildLog" in outcome
    ? `The campaign could not build ${sha}. The end of the build's output:\n\n${
      fenced(outcome.buildLog)
    }`
    : `The campaign's latest loop ran ${outcome.runs} seeds on ${sha} in ${
      (outcome.seconds / 60).toFixed(0)
    } minutes: ${
      outcome.failures === 0
        ? "no failure."
        : `${outcome.failures} failed, in the comments below.`
    }`;
  return `${SUMMARY_MARKER}\n\n${text}`;
}

// GitHub

export interface Issue {
  number: number;
  body: string;
  url: string;
}

export interface Comment {
  id: number;
  body: string;
  user: string;
}

/** What the supervisor does on GitHub; the tests use a fake. */
export interface GitHub {
  /** The identity it posts as, `<app>[bot]` for an App. */
  login: string;
  ensureLabel(
    label: { name: string; color: string; description: string },
  ): Promise<void>;
  statuses(sha: string): Promise<StatusEntry[]>;
  createStatus(sha: string, status: NewStatus): Promise<void>;
  openIssues(label: string): Promise<Issue[]>;
  createIssue(title: string, body: string, label: string): Promise<Issue>;
  updateIssue(
    number: number,
    patch: { body?: string; state?: "closed" },
  ): Promise<void>;
  comments(issue: number): Promise<Comment[]>;
  createComment(issue: number, body: string): Promise<void>;
  updateComment(id: number, body: string): Promise<void>;
  /** Open pull requests labelled `label`, with their head commits. */
  labelledPulls(label: string): Promise<{ number: number; sha: string }[]>;
}

/** Files a trusted commit's failures as issues, one per signature. */
async function fileIssue(
  github: GitHub,
  failure: Failure,
  commit: string,
): Promise<Issue> {
  const label = failure.tripwire ? LABELS.tripwire.name : LABELS.failure.name;
  const find = async () =>
    (await github.openIssues(label))
      .filter((issue) => isFor(issue.body, failure.signature))
      .sort((a, b) => a.number - b.number);
  const [issue] = await find();
  if (issue) {
    const { state, newCommit } = mergeFailure(
      decodeState(issue.body),
      failure,
      commit,
    );
    // A comment tells those watching that it still fails on a newer
    // commit; the tripwire's issue only shows it by its body, and no issue
    // gets more than a few.
    const comment = newCommit && !failure.tripwire &&
      state.comments < ISSUE_COMMENTS_MAX;
    if (comment) state.comments += 1;
    await github.updateIssue(issue.number, { body: issueBody(state) });
    if (comment) {
      await github.createComment(
        issue.number,
        `Seen again on ${commit}, ${failure.count} times in one loop.`,
      );
    }
    return issue;
  }
  const { state } = mergeFailure(undefined, failure, commit);
  const created = await github.createIssue(
    issueTitle(failure.signature),
    issueBody(state),
    label,
  );
  // Two VMs that find a new signature at once both file it; the later
  // issue gives way to the earlier one.
  const [first] = await find();
  if (first && first.number !== created.number) {
    await github.createComment(
      created.number,
      `Duplicate of #${first.number}.`,
    );
    await github.updateIssue(created.number, { state: "closed" });
    return first;
  }
  return created;
}

/** A trusted commit's report: issues, then its status. */
export async function reportTrusted(
  github: GitHub,
  sha: string,
  tally: Tally,
  complete: boolean,
): Promise<void> {
  const issues = new Map<string, Issue>();
  for (const failure of tally.failures.values()) {
    issues.set(failure.signature, await fileIssue(github, failure, sha));
  }
  const [failure] = tally.real();
  const before = foldStatuses(await github.statuses(sha), github.login);
  // A loop the runner did not finish adds no runs toward clearing; its
  // failures still count.
  const loop = complete && tally.summary
    ? {
      runs: tally.summary.runs,
      seconds: tally.summary.seconds,
      trips: tally.trips,
    }
    : { runs: 0, seconds: 0, trips: 0 };
  if (!failure && before.statuses >= STATUS_CAP) return;
  if (!failure && loop.runs === 0) return;
  await github.createStatus(
    sha,
    nextStatus(
      before,
      loop,
      failure && {
        signature: failure.signature,
        url: issues.get(failure.signature)?.url ?? "",
      },
    ),
  );
  // Another VM may have posted a failure between our read and our write,
  // and the release check reads only the latest status: a failure in the
  // history is posted again on top.
  const after = foldStatuses(await github.statuses(sha), github.login);
  if (after.failed && after.latest !== "failure") {
    await github.createStatus(
      sha,
      nextStatus(after, { runs: 0, seconds: 0, trips: 0 }),
    );
  }
}

/** A pull request's report: comments on it, and nothing else. What its
 * runner printed is untrusted, and stays on the pull request. */
export async function reportPullRequest(
  github: GitHub,
  pr: number,
  sha: string,
  result: { tally: Tally } | { buildLog: string },
): Promise<void> {
  const mine = (await github.comments(pr)).filter((comment) =>
    comment.user === github.login
  );
  const upsert = async (marker: string, body: string) => {
    const comment = mine.find((each) => firstLine(each.body, marker));
    if (comment) await github.updateComment(comment.id, body);
    else await github.createComment(pr, body);
  };
  if ("buildLog" in result) {
    await upsert(
      SUMMARY_MARKER,
      pullRequestSummary(sha, { buildLog: result.buildLog }),
    );
    return;
  }
  const { tally } = result;
  await upsert(
    SUMMARY_MARKER,
    pullRequestSummary(sha, {
      runs: tally.summary?.runs ?? tally.runs,
      seconds: tally.summary?.seconds ?? 0,
      failures: tally.real().length,
    }),
  );
  for (const failure of tally.real()) {
    const marker = signatureMarker(failure.signature);
    const known = mine.find((each) => isFor(each.body, failure.signature));
    const { state } = mergeFailure(
      known && decodeState(known.body),
      failure,
      sha,
    );
    await upsert(marker, pullRequestComment(state));
  }
}

// Targets

export type Target =
  | { kind: "main"; sha: string }
  | { kind: "release"; sha: string }
  | { kind: "pr"; sha: string; number: number };

/** Picks a loop's target by SHARES, with `random` in [0, 1). */
export function chooseTarget(candidates: Target[], random: number): Target {
  assert(random >= 0 && random < 1, "random");
  const main = candidates.filter((target) => target.kind === "main");
  assert(main.length === 1, "exactly one main");
  const pulls = candidates.filter((target) => target.kind === "pr");
  const weighted = candidates.map((target) => ({
    target,
    weight: target.kind === "pr"
      ? SHARES.pr / pulls.length
      : SHARES[target.kind],
  }));
  const total = weighted.reduce((sum, each) => sum + each.weight, 0);
  let point = random * total;
  for (const { target, weight } of weighted) {
    if (point < weight) return target;
    point -= weight;
  }
  return main[0];
}

// The sandbox

export interface SandboxSpec {
  /** A unit name, unique to this step. */
  unit: string;
  user: string;
  cwd: string;
  writable: string[];
  /** `none` for builds and runs; `public` for fetching dependencies. */
  network: "none" | "public";
  env: Record<string, string>;
  maxSeconds: number;
  command: string[];
  /** A resolv.conf naming public resolvers, for `public`: the VM's own is
   * the metadata server, which the sandbox must not reach. */
  resolvConf: string;
}

/** Variables of the supervisor's environment a sandbox does not get: its
 * own home and temporary directories, and anything that names a secret. */
const WITHHELD =
  /^(HOME|USER|LOGNAME|PWD|OLDPWD|SHLVL|TMPDIR|TMP|TEMP|TEMPDIR|NIX_BUILD_TOP|INVOCATION_ID|JOURNAL_STREAM|SYSTEMD_EXEC_PID|MANAGERPID|CREDENTIALS_DIRECTORY|SIM_CAMPAIGN_.*|SSH_.*|.*(TOKEN|SECRET|KEY|PASSWORD|CREDENTIAL).*)$/;

/** The environment of a sandboxed step: the dev shell's toolchain from the
 * supervisor's environment, without what it must not see. */
export function sandboxEnv(
  base: Record<string, string | undefined>,
  extra: Record<string, string>,
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [name, value] of Object.entries(base)) {
    if (value !== undefined && !WITHHELD.test(name)) env[name] = value;
  }
  return { ...env, ...extra };
}

/** The `systemd-run` arguments of a sandboxed step. It runs as its own
 * user, sees the system read-only but for `writable`, cannot see /root,
 * /home or other users' processes, gets private /tmp, /dev and IPC, cannot
 * gain privileges, and cannot reach the Nix daemon, whose builds run
 * outside the sandbox with network. Without network it cannot reach the
 * metadata server, whose token reads the GitHub key. */
export function sandboxArgs(spec: SandboxSpec): string[] {
  const properties = [
    `User=${spec.user}`,
    `Group=${spec.user}`,
    "ProtectSystem=strict",
    "ProtectHome=yes",
    "PrivateTmp=yes",
    "PrivateDevices=yes",
    "PrivateIPC=yes",
    "ProtectProc=invisible",
    "NoNewPrivileges=yes",
    "RestrictSUIDSGID=yes",
    "RemoveIPC=yes",
    "LockPersonality=yes",
    "RestrictRealtime=yes",
    "RestrictNamespaces=yes",
    "ProtectKernelTunables=yes",
    "ProtectKernelModules=yes",
    "ProtectKernelLogs=yes",
    "ProtectControlGroups=yes",
    "ProtectClock=yes",
    "ProtectHostname=yes",
    "CapabilityBoundingSet=",
    "InaccessiblePaths=-/nix/var/nix/daemon-socket",
    `RuntimeMaxSec=${spec.maxSeconds}`,
    "PartOf=dassie-sim-campaign.service",
    ...spec.writable.map((path) => `ReadWritePaths=${path}`),
  ];
  if (spec.network === "none") {
    properties.push("PrivateNetwork=yes");
  } else {
    // Public addresses only: not the metadata server, which is link-local
    // (and fd20:ce::254 on IPv6), nor the VPC. Names resolve through
    // public resolvers, since the VM's resolver is the metadata server.
    properties.push(
      "IPAddressDeny=localhost link-local multicast 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 fc00::/7",
      "RestrictAddressFamilies=AF_INET AF_INET6",
      `BindReadOnlyPaths=${spec.resolvConf}:/etc/resolv.conf`,
    );
  }
  return [
    "--quiet",
    "--wait",
    "--pipe",
    "--collect",
    `--unit=${spec.unit}`,
    `--working-directory=${spec.cwd}`,
    ...properties.map((property) => `--property=${property}`),
    ...Object.entries(spec.env).map(([name, value]) =>
      `--setenv=${name}=${value}`
    ),
    "--",
    ...spec.command,
  ];
}

// IO

function assert(condition: boolean, what: string): asserts condition {
  if (!condition) throw new Error(`assertion failed: ${what}`);
}

/** Keeps the last `max` characters of what is appended. */
class Tail {
  #text = "";
  #max: number;
  constructor(max: number) {
    this.#max = max;
  }
  push(text: string) {
    this.#text = (this.#text + text).slice(-this.#max);
  }
  toString() {
    return this.#text;
  }
}

/** Runs a command, passing each line it prints to `onLine` if given, and
 * resolves with its exit code and the end of its standard error. */
function run(
  command: string,
  args: string[],
  options: {
    cwd?: string;
    env?: Record<string, string>;
    onLine?: (line: string) => void;
  } = {},
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stdout = new Tail(BUILD_LOG_BYTES_MAX);
  const stderr = new Tail(BUILD_LOG_BYTES_MAX);
  if (options.onLine) {
    const onLine = options.onLine;
    // Lines are split here, not by readline, which would hold a line of
    // any length: one past LINE_BYTES_MAX is dropped, and passed on as an
    // empty line, which parses as invalid.
    let line = "";
    let skipping = false;
    child.stdout.setEncoding("utf8").on("data", (text: string) => {
      const parts = text.split("\n");
      for (const [index, part] of parts.entries()) {
        if (!skipping) line += part;
        if (line.length > LINE_BYTES_MAX) {
          line = "";
          skipping = true;
        }
        if (index === parts.length - 1) break;
        onLine(skipping ? "" : line);
        line = "";
        skipping = false;
      }
    });
    child.stdout.on("end", () => {
      if (line || skipping) onLine(skipping ? "" : line);
    });
  } else {
    child.stdout.setEncoding("utf8").on(
      "data",
      (text: string) => stdout.push(text),
    );
  }
  child.stderr.setEncoding("utf8").on(
    "data",
    (text: string) => stderr.push(text),
  );
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once(
      "close",
      (code) =>
        resolve({ code, stdout: stdout.toString(), stderr: stderr.toString() }),
    );
  });
}

async function git(repo: string, ...args: string[]): Promise<string> {
  const { code, stdout, stderr } = await run("git", ["-C", repo, ...args]);
  if (code !== 0) throw new Error(`git ${args[0]} failed: ${stderr.trim()}`);
  return stdout.trim();
}

const API = "https://api.github.com";

async function request(
  token: string | undefined,
  method: string,
  path: string,
  body?: unknown,
): Promise<Response> {
  const response = await fetch(`${API}${path}`, {
    method,
    headers: {
      accept: "application/vnd.github+json",
      "x-github-api-version": "2022-11-28",
      "user-agent": "dassie-sim-campaign",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok && response.status !== 404) {
    throw new Error(
      `GitHub ${method} ${path}: ${response.status} ${await response.text()}`,
    );
  }
  return response;
}

/** GitHub's REST API, as `login`. In a dry run it reads without a token
 * and only logs what it would write. */
class RestGitHub implements GitHub {
  readonly login: string;
  #token: string | undefined;
  #dryRun: boolean;

  constructor(login: string, token: string | undefined, dryRun: boolean) {
    this.login = login;
    this.#token = token;
    this.#dryRun = dryRun;
  }

  async #read<T>(path: string): Promise<T | undefined> {
    const response = await request(this.#token, "GET", path);
    return response.status === 404 ? undefined : (await response.json()) as T;
  }

  async #pages<T>(path: string): Promise<T[]> {
    const all: T[] = [];
    for (let page = 1; page <= PAGES_MAX; page++) {
      const separator = path.includes("?") ? "&" : "?";
      const items =
        await this.#read<T[]>(`${path}${separator}per_page=100&page=${page}`) ??
          [];
      all.push(...items);
      if (items.length < 100) break;
    }
    return all;
  }

  async #write(method: string, path: string, body: unknown): Promise<unknown> {
    if (this.#dryRun) {
      console.log(
        `dry run: ${method} ${path} ${JSON.stringify(body).slice(0, 400)}`,
      );
      return { number: 0, html_url: "", id: 0 };
    }
    return await (await request(this.#token, method, path, body)).json();
  }

  async ensureLabel(
    label: { name: string; color: string; description: string },
  ) {
    if (await this.#read(`/repos/${REPO}/labels/${label.name}`)) return;
    await this.#write("POST", `/repos/${REPO}/labels`, label);
  }

  async statuses(sha: string): Promise<StatusEntry[]> {
    type Raw = {
      state: StatusState;
      description: string | null;
      context: string;
      target_url: string | null;
      creator: { login: string } | null;
    };
    return (await this.#pages<Raw>(`/repos/${REPO}/commits/${sha}/statuses`))
      .filter((status) => status.context === STATUS_CONTEXT)
      .map((status) => ({
        state: status.state,
        description: status.description ?? "",
        creator: status.creator?.login ?? "",
        targetUrl: status.target_url ?? "",
      }));
  }

  async createStatus(sha: string, status: NewStatus) {
    await this.#write("POST", `/repos/${REPO}/statuses/${sha}`, {
      state: status.state,
      description: status.description,
      context: STATUS_CONTEXT,
      ...(status.targetUrl ? { target_url: status.targetUrl } : {}),
    });
  }

  async openIssues(label: string): Promise<Issue[]> {
    type Raw = {
      number: number;
      body: string | null;
      html_url: string;
      pull_request?: unknown;
    };
    return (await this.#pages<Raw>(
      `/repos/${REPO}/issues?state=open&labels=${label}`,
    ))
      .filter((issue) => !issue.pull_request)
      .map((issue) => ({
        number: issue.number,
        body: issue.body ?? "",
        url: issue.html_url,
      }));
  }

  async createIssue(
    title: string,
    body: string,
    label: string,
  ): Promise<Issue> {
    const issue = await this.#write("POST", `/repos/${REPO}/issues`, {
      title,
      body,
      labels: [label],
    }) as { number: number; html_url: string };
    return { number: issue.number, body, url: issue.html_url };
  }

  async updateIssue(
    number: number,
    patch: { body?: string; state?: "closed" },
  ) {
    await this.#write("PATCH", `/repos/${REPO}/issues/${number}`, patch);
  }

  async comments(issue: number): Promise<Comment[]> {
    type Raw = {
      id: number;
      body: string | null;
      user: { login: string } | null;
    };
    return (await this.#pages<Raw>(`/repos/${REPO}/issues/${issue}/comments`))
      .map(
        (comment) => ({
          id: comment.id,
          body: comment.body ?? "",
          user: comment.user?.login ?? "",
        }),
      );
  }

  async createComment(issue: number, body: string) {
    await this.#write("POST", `/repos/${REPO}/issues/${issue}/comments`, {
      body,
    });
  }

  async updateComment(id: number, body: string) {
    await this.#write("PATCH", `/repos/${REPO}/issues/comments/${id}`, {
      body,
    });
  }

  async labelledPulls(label: string) {
    type Raw = {
      number: number;
      head: { sha: string };
      labels: { name: string }[];
    };
    return (await this.#pages<Raw>(`/repos/${REPO}/pulls?state=open`))
      .filter((pull) => pull.labels.some((each) => each.name === label))
      .map((pull) => ({ number: pull.number, sha: pull.head.sha }));
  }
}

/** Reads a Secret Manager version as the VM's service account, whose
 * token only the metadata server gives, and only to a process with
 * network: never to a sandbox. */
async function readSecret(version: string): Promise<string> {
  const metadata = await fetch(
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token",
    { headers: { "metadata-flavor": "Google" } },
  );
  if (!metadata.ok) throw new Error(`metadata server: ${metadata.status}`);
  const { access_token } = await metadata.json() as { access_token: string };
  const response = await fetch(
    `https://secretmanager.googleapis.com/v1/${version}:access`,
    { headers: { authorization: `Bearer ${access_token}` } },
  );
  if (!response.ok) throw new Error(`Secret Manager: ${response.status}`);
  const { payload } = await response.json() as { payload: { data: string } };
  return Buffer.from(payload.data, "base64").toString().trim();
}

const base64url = (data: string | Buffer) =>
  Buffer.from(data).toString("base64url");

/** An installation token of the GitHub App, which lives an hour: a loop
 * mints one to choose its target and another to report. */
async function appToken(
  appId: string,
  key: string,
): Promise<{ token: string; login: string }> {
  const now = Math.floor(Date.now() / 1000);
  const unsigned = `${
    base64url(JSON.stringify({ alg: "RS256", typ: "JWT" }))
  }.${
    base64url(JSON.stringify({ iat: now - 60, exp: now + 540, iss: appId }))
  }`;
  const jwt = `${unsigned}.${
    createSign("RSA-SHA256").update(unsigned).sign(key, "base64url")
  }`;
  const app = await (await request(jwt, "GET", "/app")).json() as {
    slug: string;
  };
  const installation =
    await (await request(jwt, "GET", `/repos/${REPO}/installation`)).json() as {
      id: number;
    };
  const { token } = await (await request(
    jwt,
    "POST",
    `/app/installations/${installation.id}/access_tokens`,
    {
      repositories: [REPO.split("/")[1]],
      permissions: {
        issues: "write",
        pull_requests: "write",
        statuses: "write",
      },
    },
  )).json() as { token: string };
  return { token, login: `${app.slug}[bot]` };
}

interface Config {
  dir: string;
  repo: string;
  minutes: number;
  dryRun: boolean;
  unsandboxed: boolean;
  secret: string;
  appId: string;
}

async function connect(config: Config): Promise<GitHub> {
  if (config.dryRun) return new RestGitHub("dry-run", undefined, true);
  const secret = await readSecret(config.secret);
  if (config.appId) {
    const { token, login } = await appToken(config.appId, secret);
    return new RestGitHub(login, token, false);
  }
  // A dedicated account's fine-grained token, scoped to this repository.
  const user = await (await request(secret, "GET", "/user")).json() as {
    login: string;
  };
  return new RestGitHub(user.login, secret, false);
}

/** What can run this loop. The release candidate is the `release` branch,
 * once `main` holds its commit (so it is reviewed code) and while its
 * campaign is not settled. */
async function candidates(github: GitHub, config: Config): Promise<Target[]> {
  const main = await git(config.repo, "rev-parse", "HEAD");
  const targets: Target[] = [{ kind: "main", sha: main }];
  const fetched = await run("git", [
    "-C",
    config.repo,
    "fetch",
    "--quiet",
    "origin",
    "+refs/heads/release:refs/remotes/origin/release",
  ]);
  if (fetched.code === 0) {
    const release = await git(
      config.repo,
      "rev-parse",
      "refs/remotes/origin/release",
    );
    const onMain = await run("git", [
      "-C",
      config.repo,
      "merge-base",
      "--is-ancestor",
      release,
      main,
    ]);
    if (onMain.code !== 0) {
      console.log(`release ${release} is not on main: not run`);
    } else if (release !== main) {
      const clearing = foldStatuses(
        await github.statuses(release),
        github.login,
      );
      if (!settled(clearing)) targets.push({ kind: "release", sha: release });
    }
  }
  for (const pull of await github.labelledPulls(LABELS.optIn.name)) {
    targets.push({ kind: "pr", sha: pull.sha, number: pull.number });
  }
  return targets;
}

const SANDBOX_USER = { main: "sim-main", release: "sim-main", pr: "sim-pr" };

/** Builds and runs one target's campaign. */
async function campaign(
  target: Target,
  config: Config,
): Promise<{ tally: Tally; complete: boolean } | { buildLog: string }> {
  const slot = `${config.dir}/work/${target.kind}`;
  const src = `${slot}/src`;
  const writable = [`${slot}/target`, `${slot}/cargo`, `${slot}/home`];
  const user = SANDBOX_USER[target.kind];
  if (target.kind === "pr") {
    // A pull request starts from nothing and leaves nothing: no build of
    // one can reach a later run.
    await rm(slot, { recursive: true, force: true });
    await git(
      config.repo,
      "fetch",
      "--quiet",
      "origin",
      `+refs/pull/${target.number}/head:refs/campaign/pr`,
    );
    // A push since the pull request was listed runs the newer head.
    target.sha = await git(config.repo, "rev-parse", "refs/campaign/pr");
    assert(/^[0-9a-f]{40}$/.test(target.sha), "sha");
    await mkdir(src, { recursive: true });
    const unpacked = await run("sh", [
      "-c",
      `git -C "$1" archive "$2" | tar -x -C "$3"`,
      "sh",
      config.repo,
      target.sha,
      src,
    ]);
    if (unpacked.code !== 0) {
      throw new Error(`unpacking failed: ${unpacked.stderr}`);
    }
  } else {
    // A trusted commit keeps its build between loops; a checkout marks
    // what changed as new, so Cargo rebuilds it.
    const top = await run("git", ["-C", src, "rev-parse", "--show-toplevel"]);
    if (top.code !== 0 || top.stdout.trim() !== src) {
      await rm(src, { recursive: true, force: true });
      await mkdir(slot, { recursive: true });
      await git(config.repo, "worktree", "prune");
      await git(config.repo, "worktree", "add", "--detach", src, target.sha);
    }
    await git(src, "checkout", "--quiet", "--force", "--detach", target.sha);
  }
  for (const path of writable) {
    await mkdir(path, { recursive: true });
    if (!config.unsandboxed) await run("chown", [`${user}:${user}`, path]);
  }
  const env = sandboxEnv(process.env, {
    HOME: `${slot}/home`,
    CARGO_HOME: `${slot}/cargo`,
    CARGO_TARGET_DIR: `${slot}/target`,
  });
  const step = (
    name: string,
    network: SandboxSpec["network"],
    cwd: string,
    maxSeconds: number,
    command: string[],
    onLine?: (line: string) => void,
  ) => {
    if (config.unsandboxed) {
      return run(command[0], command.slice(1), { cwd, env, onLine });
    }
    return run(
      "systemd-run",
      sandboxArgs({
        unit: `sim-sandbox-${target.kind}-${name}-${Date.now()}`,
        user,
        cwd,
        writable,
        network,
        env,
        maxSeconds,
        command,
        resolvConf: `${config.dir}/resolv.conf`,
      }),
      { onLine },
    );
  };
  // Fetching runs no code of the commit's, and runs outside its tree, so
  // that no `.cargo/config.toml` of a pull request applies to it.
  const fetched = await step("fetch", "public", `${slot}/home`, 30 * 60, [
    "cargo",
    "fetch",
    "--locked",
    "--manifest-path",
    `${src}/Cargo.toml`,
  ]);
  if (fetched.code !== 0) return { buildLog: fetched.stderr };
  const built = await step("build", "none", src, 60 * 60, [
    "cargo",
    "build",
    "--locked",
    "--offline",
    "--profile",
    "sim",
    "-p",
    "dassie-sim",
  ]);
  if (built.code !== 0) return { buildLog: built.stderr };
  const tally = new Tally();
  const ran = await step(
    "run",
    "none",
    `${slot}/home`,
    (config.minutes + 15) * 60,
    [
      `${slot}/target/sim/dassie-sim`,
      "campaign",
      "--minutes",
      String(config.minutes),
      "--jobs",
      String(availableParallelism()),
    ],
    (line) => tally.add(parseLine(line)),
  );
  if (tally.invalid > 0) {
    console.log(`${tally.invalid} lines of output were invalid`);
  }
  if (ran.code !== 0) {
    console.log(`the runner exited ${ran.code}: ${ran.stderr.slice(-2000)}`);
  }
  if (target.kind === "pr") await rm(slot, { recursive: true, force: true });
  return { tally, complete: ran.code === 0 && tally.summary !== undefined };
}

async function loop(config: Config): Promise<void> {
  if (!config.unsandboxed) {
    // A loop that died left its sandboxes running; nothing else should.
    await run("systemctl", ["stop", "sim-sandbox-*.service"]);
  }
  let github = await connect(config);
  for (const label of Object.values(LABELS)) await github.ensureLabel(label);
  const target = chooseTarget(await candidates(github, config), Math.random());
  console.log(`running ${target.kind} ${target.sha}`);
  const result = await campaign(target, config);
  // The token chosen with may have expired over the hour.
  github = await connect(config);
  if (target.kind === "pr") {
    await reportPullRequest(github, target.number, target.sha, result);
  } else if ("buildLog" in result) {
    // CI keeps main building; a trusted commit that fails to build here
    // gets no status, rather than one about the VM.
    console.log(`${target.sha} did not build: ${result.buildLog.slice(-2000)}`);
  } else {
    await reportTrusted(github, target.sha, result.tally, result.complete);
  }
}

const USAGE =
  "usage: sim-campaign.ts [--minutes N] [--dry-run [--unsandboxed]]";

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let args;
  try {
    args = parseArgs({
      options: {
        minutes: { type: "string", default: "60" },
        "dry-run": { type: "boolean", default: false },
        unsandboxed: { type: "boolean", default: false },
      },
    });
  } catch (error) {
    console.error(`${(error as Error).message}\n${USAGE}`);
    process.exit(2);
  }
  const { values } = args;
  const minutes = Number(values.minutes);
  if (!Number.isSafeInteger(minutes) || minutes < 1 || minutes > 24 * 60) {
    console.error(USAGE);
    process.exit(2);
  }
  if (values.unsandboxed && !values["dry-run"]) {
    console.error(
      "--unsandboxed runs pull requests' code unconfined: only in a dry run",
    );
    process.exit(2);
  }
  const secret = process.env.SIM_CAMPAIGN_SECRET ?? "";
  if (!secret && !values["dry-run"]) {
    console.error("SIM_CAMPAIGN_SECRET names no secret");
    process.exit(2);
  }
  await loop({
    dir: process.env.SIM_CAMPAIGN_DIR ?? "/var/lib/dassie-sim-campaign",
    repo: fileURLToPath(new URL("../", import.meta.url)),
    minutes,
    dryRun: values["dry-run"],
    unsandboxed: values.unsandboxed,
    secret,
    appId: process.env.SIM_CAMPAIGN_APP_ID ?? "",
  });
}
