// The simulation campaign's supervisor
// (spec/decisions/random-seed-simulations.md): one loop of it, on a campaign
// VM.
//
//   node scripts/sim-campaign.ts [--minutes N] [--dry-run [--unsandboxed]]
//
// A loop picks what to run: `main`, or an open pull request labelled `sim`.
// It builds that commit's `dassie-sim` and runs `dassie-sim campaign` on
// every core for an hour, then reports: `main` gets one issue per failure
// signature; a pull request gets comments on itself, and nothing else.
// Neither gets a commit status: the campaign is not a check.
//
// The loop then exits, and systemd starts the next one from a fresh
// checkout of `main` (scripts/sim-campaign/dassie-sim-campaign.service), so
// a change to this script, or to the runner, reaches every VM within a loop.
// A loop that cannot build or start `main` reports the campaign's
// own trouble in an issue of its own, and exits with an error.
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
import { availableParallelism, hostname } from "node:os";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const REPO = "justmoon/dassie";

/** The shares of loops, by what they run. The pull requests' share is
 * split among them. */
export const SHARES = { main: 2, pr: 1 };

/** The labels the campaign uses, created if missing. */
export const LABELS = {
  failure: {
    name: "sim-failure",
    color: "b60205",
    description:
      "A failure the simulation campaign found (spec/decisions/random-seed-simulations.md)",
  },
  tripwire: {
    name: "sim-tripwire",
    color: "fbca04",
    description:
      "The campaign's tripwire, which fails on purpose (spec/decisions/random-seed-simulations.md)",
  },
  optIn: {
    name: "sim",
    color: "1d76db",
    description:
      "Run this pull request in the simulation campaign (spec/decisions/random-seed-simulations.md)",
  },
  trouble: {
    name: "sim-campaign",
    color: "5319e7",
    description:
      "The simulation campaign itself is in trouble (spec/decisions/random-seed-simulations.md)",
  },
};

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
/** A sandbox's share of the VM's memory: what is left keeps the system and
 * the supervisor alive when a step runs out, so the loop still reports. */
const SANDBOX_MEMORY_MAX = "80%";
/** A sandbox's processes and threads: far more than a build on every core
 * or a campaign with a job per core needs, and few enough that a fork bomb
 * cannot starve the supervisor. */
const SANDBOX_TASKS_MAX = 4096;

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
  /** Runs that broke safety or liveness, the tripwire's included. */
  failures: number;
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
    const failures = count(summary?.failures);
    const seconds = typeof summary?.seconds === "number" &&
        Number.isFinite(summary.seconds) && summary.seconds >= 0
      ? summary.seconds
      : undefined;
    if (
      runs === undefined || failures === undefined || seconds === undefined
    ) {
      return { kind: "invalid", reason: "malformed summary" };
    }
    return { kind: "summary", summary: { runs, seconds, failures } };
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
  invalid = 0;
  /** A last line with no newline that did not parse: what a runner
   * stopped while it wrote leaves. */
  cut = 0;
  /** Runs that failed, the tripwire's included. */
  failed = 0;
  /** Failures past SIGNATURES_MAX distinct signatures. */
  dropped = 0;
  failures = new Map<string, Failure>();
  summary: Summary | undefined;

  /** Adds a line; `unterminated` for a last one that had no newline. */
  add(line: Line, unterminated = false): void {
    if (line.kind === "invalid") {
      if (unterminated) this.cut += 1;
      else this.invalid += 1;
      return;
    }
    if (line.kind === "summary") {
      this.summary = line.summary;
      return;
    }
    const { run } = line;
    this.runs += 1;
    if (!isFailure(run)) return;
    this.failed += 1;
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

  /** The lines read, whether they parsed or not. */
  lines(): number {
    return this.runs + this.invalid + this.cut + (this.summary ? 1 : 0);
  }

  /** The failures that count against a commit: all but the tripwire's. */
  real(): Failure[] {
    return [...this.failures.values()].filter((failure) => !failure.tripwire);
  }

  /** Why some of the loop went unread, if it did. A line that did not
   * parse, a failure past what a loop keeps, or counts that disagree with
   * the runner's summary could each hide a failure, so a loop on `main`
   * fails closed on them (`unreadableFailure`). The one thing excused is
   * the last line of a loop that did not finish, cut short: a runner
   * stopped at its sandbox's limits leaves one, through no fault of the
   * commit's. */
  unreadable(finished = true): string | undefined {
    const reasons = [];
    if (this.invalid > 0) {
      reasons.push(`${this.invalid} lines of output did not parse`);
    }
    if (finished && this.cut > 0) {
      reasons.push("the last line of output did not parse");
    }
    if (this.dropped > 0) {
      reasons.push(
        `${this.dropped} failures came past the ${SIGNATURES_MAX} signatures a loop keeps`,
      );
    }
    if (this.summary && this.summary.runs !== this.runs) {
      reasons.push(
        `the summary counts ${this.summary.runs} runs, the lines ${this.runs}`,
      );
    }
    if (this.summary && this.summary.failures !== this.failed) {
      reasons.push(
        `the summary counts ${this.summary.failures} failures, the lines ${this.failed}`,
      );
    }
    return reasons.length > 0 ? reasons.join("; ") : undefined;
  }
}

/** The signature `main` fails under when its loop went unread. */
export const UNREADABLE = "campaign/unreadable";

/** The failure that stands for what a loop could not read. */
export function unreadableFailure(reason: string): Failure {
  return {
    signature: UNREADABLE,
    tripwire: false,
    count: 1,
    fastest: {
      sim: "campaign",
      seed: "0",
      outcome: "safety",
      signature: UNREADABLE,
      ms: 0,
      message: reason,
      options: "",
      traceTail: "",
    },
  };
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

function encodeState(state: FailureState | TroubleState): string {
  const data = Buffer.from(JSON.stringify(state)).toString("base64");
  return `<!-- sim-campaign state ${data} -->`;
}

/** The object in a body's state, if it has one that parses. */
function stateData(body: string): Record<string, unknown> | undefined {
  const match = body.match(STATE);
  if (!match) return undefined;
  try {
    const data = JSON.parse(Buffer.from(match[1], "base64").toString());
    return typeof data === "object" && data !== null && !Array.isArray(data)
      ? data
      : undefined;
  } catch {
    return undefined;
  }
}

/** The state in a body, if it has one that parses: people edit bodies. */
export function decodeState(body: string): FailureState | undefined {
  const state = stateData(body);
  if (
    typeof state?.signature !== "string" || count(state.seen) === undefined ||
    !Array.isArray(state.commits) ||
    typeof (state.fastest as { seed?: unknown } | undefined)?.seed !== "string"
  ) {
    return undefined;
  }
  return state as unknown as FailureState;
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
  if (state.signature === UNREADABLE) {
    return [
      `The runner's output on ${fastest.commit} could not all be read, and a failure may hide in what was lost: ${fastest.message}. To see it, run the campaign at that commit:`,
      fenced(
        `git checkout ${fastest.commit}\ncargo run -p dassie-sim --profile sim -- campaign --minutes 10`,
        "sh",
      ),
    ].join("\n\n");
  }
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
    : "The simulation campaign found this failure on `main`.";
  return [
    signatureMarker(state.signature),
    `${intro} Its signature is \`${state.signature}\`.`,
    `Seen ${state.seen} times, latest on ${state.commits.at(-1)} (commits: ${
      state.commits.join(", ")
    }).`,
    details(state),
    "A seed reproduces a failure only at the commit it ran on, so a fix comes with a targeted test (spec/decisions/random-seed-simulations.md).",
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
  outcome: {
    runs: number;
    seconds: number;
    failures: number;
    unreadable?: string;
  } | {
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
    }${
      outcome.unreadable
        ? ` Not all of its output could be read, and a failure may hide in what was lost: ${outcome.unreadable}.`
        : ""
    }`;
  return `${SUMMARY_MARKER}\n\n${text}`;
}

// The campaign's own trouble

/** The signature of a commit of `main` the campaign could not build or
 * start. */
export const TROUBLE = "campaign/build";

/** Where a loop on `main` stopped short of its runs. */
export type TroubleStep = "prepare" | "fetch" | "build" | "run";

const TROUBLE_STEPS: Record<TroubleStep, string> = {
  prepare: "checking out",
  fetch: "fetching the dependencies of",
  build: "building",
  run: "starting the runner of",
};

/** One loop's trouble with a commit of `main`. */
export interface Trouble {
  sha: string;
  step: TroubleStep;
  log: string;
  /** When, as an ISO time. */
  time: string;
  /** The VM it happened on. */
  host: string;
}

/** What the trouble issue keeps, as a failure's issue does. */
export interface TroubleState {
  signature: string;
  seen: number;
  first: string;
  /** The latest commits it was seen on, oldest first. */
  commits: string[];
  latest: Trouble;
}

/** The state in a trouble issue's body, if it has one that parses. */
export function decodeTrouble(body: string): TroubleState | undefined {
  const state = stateData(body);
  const latest = state?.latest as Partial<Trouble> | undefined;
  if (
    state?.signature !== TROUBLE || count(state.seen) === undefined ||
    typeof state.first !== "string" || !Array.isArray(state.commits) ||
    typeof latest?.time !== "string" || typeof latest.log !== "string"
  ) {
    return undefined;
  }
  return state as unknown as TroubleState;
}

/** Adds one loop's trouble to what was known. */
export function mergeTrouble(
  known: TroubleState | undefined,
  trouble: Trouble,
): TroubleState {
  const latest = { ...trouble, log: trouble.log.slice(-BUILD_LOG_BYTES_MAX) };
  if (!known) {
    return {
      signature: TROUBLE,
      seen: 1,
      first: trouble.time,
      commits: [trouble.sha],
      latest,
    };
  }
  const commits = known.commits.includes(trouble.sha)
    ? known.commits
    : [...known.commits, trouble.sha].slice(-COMMITS_MAX);
  return { ...known, seen: known.seen + 1, commits, latest };
}

export function troubleBody(state: TroubleState): string {
  const { latest } = state;
  return [
    signatureMarker(TROUBLE),
    "The simulation campaign could not build or start a commit of `main`. CI keeps `main` building, so this is most likely the campaign's own trouble rather than Dassie's: its sandbox, its toolchain, a VM's disk, or its network to crates.io. It is not a `sim-failure`, since nothing in it is the commit's fault.",
    `Seen ${state.seen} times since ${state.first}; latest at ${latest.time} on ${latest.host}, while ${
      TROUBLE_STEPS[latest.step]
    } main ${latest.sha} (commits: ${state.commits.join(", ")}).`,
    "The end of its log:",
    fenced(latest.log),
    "After each, the supervisor exits with an error, so `systemctl status dassie-sim-campaign` on the VM shows the unit failing, and systemd starts the next loop a minute later. Later loops update this issue in place. Close it once the campaign is fixed: it is never reopened, and trouble after that gets a new issue.",
    encodeState(state),
  ].join("\n\n");
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

/** Files or updates the open issue of `signature` under `label`: `render`
 * makes its body from the one it has, if any, and may add a comment. */
async function upsertIssue(
  github: GitHub,
  label: string,
  signature: string,
  title: string,
  render: (body: string | undefined) => { body: string; comment?: string },
): Promise<Issue> {
  const find = async () =>
    (await github.openIssues(label))
      .filter((issue) => isFor(issue.body, signature))
      .sort((a, b) => a.number - b.number);
  const [issue] = await find();
  if (issue) {
    const { body, comment } = render(issue.body);
    await github.updateIssue(issue.number, { body });
    if (comment) await github.createComment(issue.number, comment);
    return issue;
  }
  const created = await github.createIssue(
    title,
    render(undefined).body,
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

/** Files a failure on `main` as an issue, one per signature. */
function fileIssue(
  github: GitHub,
  failure: Failure,
  commit: string,
): Promise<Issue> {
  const label = failure.tripwire ? LABELS.tripwire.name : LABELS.failure.name;
  return upsertIssue(
    github,
    label,
    failure.signature,
    issueTitle(failure.signature),
    (body) => {
      const { state, newCommit } = mergeFailure(
        body === undefined ? undefined : decodeState(body),
        failure,
        commit,
      );
      // A comment tells those watching that it still fails on a newer
      // commit; the tripwire's issue only shows it by its body, and no
      // issue gets more than a few.
      const comment = body !== undefined && newCommit && !failure.tripwire &&
        state.comments < ISSUE_COMMENTS_MAX;
      if (comment) state.comments += 1;
      return {
        body: issueBody(state),
        comment: comment
          ? `Seen again on ${commit}, ${failure.count} times in one loop.`
          : undefined,
      };
    },
  );
}

/** Reports a loop that could not build or start `main`: one issue for
 * all of them, under a label of its own, updated in place and never
 * commented on, since the supervisor retries every minute. It is not a
 * `sim-failure`, since the commit is not at fault. */
export function reportTrouble(
  github: GitHub,
  trouble: Trouble,
): Promise<Issue> {
  return upsertIssue(
    github,
    LABELS.trouble.name,
    TROUBLE,
    "sim-campaign: the campaign cannot build main",
    (body) => ({
      body: troubleBody(
        mergeTrouble(
          body === undefined ? undefined : decodeTrouble(body),
          trouble,
        ),
      ),
    }),
  );
}

/** Whether a loop on `main` stopped short of its runs through the
 * campaign's own trouble: it did not build, or its runner failed before it
 * printed a line, which a sandbox that could not start leaves. A runner
 * that printed something and then failed is the commit's to answer for, as
 * `reportMain` does. */
export function troubleOf(
  result: CampaignResult,
): { step: TroubleStep; log: string } | undefined {
  if ("buildLog" in result) return { step: result.step, log: result.buildLog };
  if (result.code !== 0 && result.tally.lines() === 0) {
    return { step: "run", log: result.log };
  }
  return undefined;
}

/** A loop's report on `main`: one issue per failure signature, and one
 * for what a loop could not read, if it could not read all of it. */
export async function reportMain(
  github: GitHub,
  sha: string,
  tally: Tally,
  complete: boolean,
): Promise<void> {
  const unreadable = tally.unreadable(complete);
  const failures = [...tally.failures.values()];
  if (unreadable) failures.push(unreadableFailure(unreadable));
  for (const failure of failures) await fileIssue(github, failure, sha);
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
      unreadable: tally.unreadable(),
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
    // The system calls a service needs, and nothing that changes the
    // system: less of the kernel between a pull request and root. A call
    // outside the set fails, rather than killing the step unexplained.
    "SystemCallFilter=@system-service",
    "SystemCallErrorNumber=EPERM",
    "SystemCallArchitectures=native",
    `MemoryMax=${SANDBOX_MEMORY_MAX}`,
    `TasksMax=${SANDBOX_TASKS_MAX}`,
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
    // systemd-run takes the keywords only as a whole value, unlike a unit
    // file, so each gets its own property; the properties add up.
    properties.push(
      "IPAddressDeny=localhost",
      "IPAddressDeny=link-local",
      "IPAddressDeny=multicast",
      "IPAddressDeny=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 fc00::/7",
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
    onLine?: (line: string, unterminated: boolean) => void;
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
        onLine(skipping ? "" : line, false);
        line = "";
        skipping = false;
      }
    });
    // The only line passed on without its newline: the last, if the
    // command stopped while it wrote it.
    child.stdout.on("end", () => {
      if (line || skipping) onLine(skipping ? "" : line, true);
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
      // Comments on a pull request are issue comments: reading pull
      // requests is enough, and a token that cannot edit, close or review
      // them is worth less to whoever steals it.
      permissions: {
        issues: "write",
        pull_requests: "read",
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

/** What can run this loop: `main`, and the pull requests labelled for
 * the campaign. */
async function candidates(github: GitHub, config: Config): Promise<Target[]> {
  const main = await git(config.repo, "rev-parse", "HEAD");
  const targets: Target[] = [{ kind: "main", sha: main }];
  for (const pull of await github.labelledPulls(LABELS.optIn.name)) {
    targets.push({ kind: "pr", sha: pull.sha, number: pull.number });
  }
  return targets;
}

const SANDBOX_USER = { main: "sim-main", pr: "sim-pr" };

/** Builds and runs one target's campaign. */
async function campaign(
  target: Target,
  config: Config,
): Promise<CampaignResult> {
  const slot = `${config.dir}/work/${target.kind}`;
  if (target.kind !== "pr") return await buildAndRun(target, config, slot);
  // A pull request starts from nothing and leaves nothing, however its
  // loop ends: no build of one can reach a later run, nor leave the disk
  // full for it. A supervisor that died mid-loop leaves it to the next
  // loop, which clears it first (`loop`).
  await rm(slot, { recursive: true, force: true });
  try {
    return await buildAndRun(target, config, slot);
  } finally {
    await rm(slot, { recursive: true, force: true });
  }
}

/** A loop's runs, with the runner's exit code and the end of its standard
 * error, or the end of the log of the step that failed before them. */
export type CampaignResult =
  | { tally: Tally; complete: boolean; code: number | null; log: string }
  | { buildLog: string; step: TroubleStep };

async function buildAndRun(
  target: Target,
  config: Config,
  slot: string,
): Promise<CampaignResult> {
  const src = `${slot}/src`;
  const writable = [`${slot}/target`, `${slot}/cargo`, `${slot}/home`];
  const user = SANDBOX_USER[target.kind];
  if (target.kind === "pr") {
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
    onLine?: (line: string, unterminated: boolean) => void,
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
  if (fetched.code !== 0) return { buildLog: fetched.stderr, step: "fetch" };
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
  if (built.code !== 0) return { buildLog: built.stderr, step: "build" };
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
    (line, unterminated) => tally.add(parseLine(line), unterminated),
  );
  const unreadable = tally.unreadable(
    ran.code === 0 && tally.summary !== undefined,
  );
  if (unreadable) console.log(`not all output was read: ${unreadable}`);
  if (ran.code !== 0) {
    console.log(`the runner exited ${ran.code}: ${ran.stderr.slice(-2000)}`);
  }
  return {
    tally,
    complete: ran.code === 0 && tally.summary !== undefined,
    code: ran.code,
    log: ran.stderr,
  };
}

/** One loop. It resolves `false` when the campaign could not build or start
 * `main`, which it has reported. */
async function loop(config: Config): Promise<boolean> {
  if (!config.unsandboxed) {
    // A loop that died left its sandboxes running; nothing else should.
    await run("systemctl", ["stop", "sim-sandbox-*.service"]);
  }
  await rm(`${config.dir}/work/pr`, { recursive: true, force: true });
  let github = await connect(config);
  for (const label of Object.values(LABELS)) await github.ensureLabel(label);
  const target = chooseTarget(await candidates(github, config), Math.random());
  console.log(`running ${target.kind} ${target.sha}`);
  if (target.kind === "pr") {
    const result = await campaign(target, config);
    // The token chosen with may have expired over the hour.
    github = await connect(config);
    await reportPullRequest(github, target.number, target.sha, result);
    return true;
  }
  let result: CampaignResult;
  try {
    result = await campaign(target, config);
  } catch (error) {
    // A checkout of `main` fails only when the VM's disk or Git does.
    result = {
      buildLog: String((error as Error).stack ?? error),
      step: "prepare",
    };
  }
  github = await connect(config);
  const trouble = troubleOf(result);
  if (trouble) {
    // CI keeps main building, so a commit of main that does not build or
    // start here almost always means the campaign is broken: its sandbox,
    // toolchain, disk or network. That goes to an issue of its own, never
    // to a `sim-failure`, since the commit is not at fault.
    console.log(
      `${target.kind} ${target.sha}: ${trouble.step} failed: ${
        trouble.log.slice(-2000)
      }`,
    );
    const issue = await reportTrouble(github, {
      sha: target.sha,
      ...trouble,
      time: new Date().toISOString(),
      host: hostname(),
    });
    console.log(`reported in ${issue.url}`);
    return false;
  }
  assert(!("buildLog" in result), "a build that failed is trouble");
  await reportMain(github, target.sha, result.tally, result.complete);
  return true;
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
  const worked = await loop({
    dir: process.env.SIM_CAMPAIGN_DIR ?? "/var/lib/dassie-sim-campaign",
    repo: fileURLToPath(new URL("../", import.meta.url)),
    minutes,
    dryRun: values["dry-run"],
    unsandboxed: values.unsandboxed,
    secret,
    appId: process.env.SIM_CAMPAIGN_APP_ID ?? "",
  });
  // A loop in trouble fails its unit, so that `systemctl status` shows it;
  // systemd starts the next one all the same (Restart=always).
  if (!worked) process.exitCode = 1;
}
