// scripts/sim-campaign.ts's logic against fixtures and a GitHub in memory:
// no network, no sandbox.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  chooseTarget,
  cleanSignature,
  CLEAR_RUNS,
  type Comment,
  decodeState,
  fenced,
  foldStatuses,
  type GitHub,
  type Issue,
  issueBody,
  LABELS,
  mergeFailure,
  type NewStatus,
  nextStatus,
  parseLine,
  reportPullRequest,
  reportTrusted,
  type RunRecord,
  sandboxArgs,
  sandboxEnv,
  type SandboxSpec,
  STATUS_CONTEXT,
  type StatusEntry,
  Tally,
  type Target,
} from "./sim-campaign.ts";

const BOT = "dassie-sim-campaign[bot]";
const MAIN = "a".repeat(40);
const NEXT = "b".repeat(40);

/** A line as `dassie-sim campaign` prints it. */
function line(fields: Record<string, unknown>): string {
  return JSON.stringify({
    sim: "node",
    seed: 7,
    outcome: "pass",
    signature: "node/pass/",
    ms: 1000,
    counts: {},
    ...fields,
  });
}

function failing(
  sim: string,
  seed: string,
  ms: number,
  where = "books",
): string {
  const outcome = sim === "tripwire" ? "safety" : "liveness";
  return line({
    sim,
    outcome,
    ms,
    signature: `${sim}/${outcome}/${where}`,
    message: `${sim} failed`,
    options: "nodes=4 loss=0.1",
    trace_tail: "t=1 crash\nt=2 heal",
  }).replace('"seed":7', `"seed":${seed}`);
}

/** A loop's tally of `lines`, then passing runs up to the summary's count,
 * then a summary that agrees with them all. */
function tally(lines: string[], summary?: { runs: number; seconds: number }) {
  const result = new Tally();
  for (const each of lines) result.add(parseLine(each));
  if (summary) {
    while (result.runs < summary.runs) result.add(parseLine(line({})));
    result.add(parseLine(JSON.stringify({
      summary: { ...summary, by_sim: {}, failures: result.failed },
    })));
  }
  return result;
}

/** GitHub in memory, recording every write. Its calls answer at once. */
class FakeGitHub implements GitHub {
  login = BOT;
  statusesBySha = new Map<string, StatusEntry[]>();
  issues:
    (Issue & { label: string; state: "open" | "closed"; title: string })[] = [];
  commentsByIssue = new Map<number, Comment[]>();
  writes: string[] = [];
  #next = 100;
  /** Runs after each status write, as another VM would. */
  afterStatus: ((sha: string) => void) | undefined;

  ensureLabel(label: { name: string }) {
    this.writes.push(`label ${label.name}`);
    return Promise.resolve();
  }
  statuses(sha: string) {
    return Promise.resolve([...(this.statusesBySha.get(sha) ?? [])]);
  }
  createStatus(sha: string, status: NewStatus) {
    assert(status.description.length <= 140, status.description);
    this.writes.push(`status ${sha} ${status.state}`);
    this.statusesBySha.set(sha, [
      {
        state: status.state,
        description: status.description,
        creator: this.login,
        targetUrl: status.targetUrl ?? "",
      },
      ...(this.statusesBySha.get(sha) ?? []),
    ]);
    this.afterStatus?.(sha);
    return Promise.resolve();
  }
  openIssues(label: string) {
    return Promise.resolve(
      this.issues.filter((issue) =>
        issue.label === label && issue.state === "open"
      ),
    );
  }
  createIssue(title: string, body: string, label: string) {
    const number = this.#next++;
    this.writes.push(`issue ${number} ${label}`);
    const issue = {
      number,
      body,
      title,
      label,
      state: "open" as const,
      url: `https://github.com/justmoon/dassie/issues/${number}`,
    };
    this.issues.push(issue);
    return Promise.resolve(issue);
  }
  updateIssue(number: number, patch: { body?: string; state?: "closed" }) {
    this.writes.push(`update ${number}${patch.state ? ` ${patch.state}` : ""}`);
    const issue = this.issues.find((each) => each.number === number)!;
    Object.assign(issue, patch);
    return Promise.resolve();
  }
  comments(issue: number) {
    return Promise.resolve([...(this.commentsByIssue.get(issue) ?? [])]);
  }
  createComment(issue: number, body: string) {
    this.writes.push(`comment ${issue}`);
    const comments = this.commentsByIssue.get(issue) ?? [];
    comments.push({ id: this.#next++, body, user: this.login });
    this.commentsByIssue.set(issue, comments);
    return Promise.resolve();
  }
  updateComment(id: number, body: string) {
    this.writes.push(`edit ${id}`);
    for (const comments of this.commentsByIssue.values()) {
      const comment = comments.find((each) => each.id === id);
      if (comment) comment.body = body;
    }
    return Promise.resolve();
  }
  labelledPulls() {
    return Promise.resolve([]);
  }
}

test("records: a run, a summary, and a u64 seed kept exact", () => {
  const parsed = parseLine(failing("node", "18446744073709551615", 1200));
  assert(parsed.kind === "run");
  assertEquals(parsed.run.seed, "18446744073709551615");
  assertEquals(parsed.run.outcome, "liveness");
  assertEquals(parsed.run.signature, "node/liveness/books");
  assertEquals(parsed.run.traceTail, "t=1 crash\nt=2 heal");
  assertEquals(
    parseLine(
      '{"summary":{"runs":12,"by_sim":{},"failures":1,"seconds":3600.5}}',
    ),
    { kind: "summary", summary: { runs: 12, seconds: 3600.5, failures: 1 } },
  );
  const quoted = parseLine(line({ seed: "42" }));
  assert(quoted.kind === "run");
  assertEquals(quoted.run.seed, "42");
});

test("records: anything unexpected is an invalid line, never an exception", () => {
  for (
    const each of [
      "",
      "not json",
      "[1]",
      "null",
      line({ outcome: "maybe" }),
      line({ seed: -1 }),
      line({ seed: 1.5 }),
      line({ seed: "18446744073709551616" }),
      line({ sim: "Node With Spaces" }),
      line({ ms: -3 }),
      line({ signature: 3 }),
      '{"summary":{"runs":"many","seconds":1,"failures":0}}',
      '{"summary":{"runs":1,"seconds":1}}',
      " ".repeat(2 << 20),
    ]
  ) {
    assertEquals(parseLine(each).kind, "invalid", each.slice(0, 60));
  }
});

test("signatures: what a run prints cannot break out of a marker or a code span", () => {
  assertEquals(
    cleanSignature("node/safety/a --> `b`"),
    "node/safety/a_--___b_",
  );
  assertEquals(cleanSignature("x".repeat(500)).length, 200);
});

test("a loop's tally: runs, failures by signature, the fastest seed, trips", () => {
  const result = tally([
    line({}),
    failing("node", "5", 9000),
    failing("node", "6", 300),
    failing("node", "7", 4000),
    failing("tripwire", "8", 10, "tripwire"),
    line({ sim: "peer", outcome: "unsupported" }),
    "garbage",
  ], { runs: 6, seconds: 3600 });
  assertEquals(result.runs, 6);
  assertEquals(result.invalid, 1);
  assertEquals(result.trips, 1);
  const node = result.failures.get("node/liveness/books")!;
  assertEquals(node.count, 3);
  assertEquals(node.fastest.seed, "6");
  assertEquals(result.real().map((each) => each.signature), [
    "node/liveness/books",
  ]);
  assertEquals(result.unreadable(), "1 lines of output did not parse");
});

test("a loop is unread when a line fails to parse, a failure is dropped, or the summary disagrees", () => {
  const clean = tally([failing("node", "1", 5)], { runs: 10, seconds: 60 });
  assertEquals(clean.unreadable(), undefined);

  const tooLong = tally([
    failing("node", "1", 5).replace(
      '"message":',
      `"pad":"${"x".repeat(1 << 20)}","message":`,
    ),
  ]);
  assertEquals(tooLong.unreadable(), "1 lines of output did not parse");

  const many = tally(
    Array.from(
      { length: 65 },
      (_, i) => failing("node", String(i), 5, `check${i}`),
    ),
  );
  assertEquals(many.failures.size, 64);
  assertEquals(
    many.unreadable(),
    "1 failures came past the 64 signatures a loop keeps",
  );

  const mismatch = tally([failing("node", "1", 5), line({})]);
  mismatch.add(
    parseLine('{"summary":{"runs":3,"by_sim":{},"failures":2,"seconds":1}}'),
  );
  assertEquals(
    mismatch.unreadable(),
    "the summary counts 3 runs, the lines 2; the summary counts 2 failures, the lines 1",
  );
});

test("status: a trusted loop not all read fails closed, and adds no runs", async () => {
  const github = new FakeGitHub();
  // Only the tripwire's failures parsed; a real one was lost in a line too
  // long to read.
  const loop = tally([failing("tripwire", "3", 10, "tripwire"), "{not json"]);
  loop.add(
    parseLine('{"summary":{"runs":2,"by_sim":{},"failures":2,"seconds":3600}}'),
  );
  await reportTrusted(github, MAIN, loop, true);
  const [status] = github.statusesBySha.get(MAIN)!;
  assertEquals(status.state, "failure");
  assert(status.description.startsWith("failed: campaign/unreadable"));
  assertEquals(foldStatuses([status], BOT).runs, 0);
  const issue = github.issues.find((each) =>
    each.label === LABELS.failure.name
  )!;
  assertEquals(issue.title, "sim: campaign/unreadable");
  assertEquals(status.targetUrl, issue.url);
  assert(issue.body.includes("did not parse"));
  assert(issue.body.includes("-- campaign --minutes"));
});

/** A loop's tally of `lines`, ended by `cut`, a line the runner was
 * stopped while it wrote, with no newline and no summary. */
function cutShort(lines: string[], cut: string) {
  const result = tally(lines);
  result.add(parseLine(cut), true);
  return result;
}

const CUT = failing("node", "4", 10).slice(0, 40);

test("status: an unfinished loop's last line, cut short, fails nothing, and adds no runs", async () => {
  const github = new FakeGitHub();
  await reportTrusted(github, MAIN, cutShort([line({})], CUT), false);
  assertEquals(github.issues.length, 0);
  assertEquals(github.statusesBySha.get(MAIN), undefined);
  // Its real failures still count.
  await reportTrusted(
    github,
    MAIN,
    cutShort([failing("node", "5", 10)], CUT),
    false,
  );
  assertEquals(github.issues.map((each) => each.title), [
    "sim: node/liveness/books",
  ]);
  const [status] = github.statusesBySha.get(MAIN)!;
  assertEquals(status.state, "failure");
  assertEquals(foldStatuses([status], BOT).runs, 0);
});

test("status: an unfinished loop with a line it could not read before its cut last line fails closed", async () => {
  const github = new FakeGitHub();
  // A fully written line that did not parse may be a failure: the kill
  // that cut the last line does not explain it.
  await reportTrusted(
    github,
    MAIN,
    cutShort([line({}), "{not json"], CUT),
    false,
  );
  const [status] = github.statusesBySha.get(MAIN)!;
  assertEquals(status.state, "failure");
  assert(status.description.startsWith("failed: campaign/unreadable"));
  assertEquals(foldStatuses([status], BOT).runs, 0);
  const issue = github.issues.find((each) =>
    each.title === "sim: campaign/unreadable"
  )!;
  assert(issue.body.includes("1 lines of output did not parse"));
  assert(!issue.body.includes("the last line"));
});

test("a finished loop is not excused its cut last line", () => {
  const loop = cutShort([line({})], CUT);
  assertEquals(loop.unreadable(false), undefined);
  assertEquals(loop.unreadable(true), "the last line of output did not parse");
});

function entry(
  state: StatusEntry["state"],
  description: string,
  creator = BOT,
): StatusEntry {
  return { state, description, creator, targetUrl: "" };
}

test("status: the history adds up each loop's own counts, from the campaign alone", () => {
  const clearing = foldStatuses([
    entry("pending", "200 runs of 30000, 0.1 h, 0 trips [+100 +200s +0t]"),
    entry("success", "cleared: forged [+99999 +1s +9t]", "someone-else"),
    entry("pending", "100 runs of 30000, 0.1 h, 1 trips [+100 +300s +1t]"),
    entry("pending", "set by hand"),
  ], BOT);
  assertEquals(
    { runs: clearing.runs, seconds: clearing.seconds, trips: clearing.trips },
    { runs: 200, seconds: 500, trips: 1 },
  );
  assertEquals(clearing.statuses, 3);
  assertEquals(clearing.failed, false);
});

test("status: a failure anywhere in the history is final", () => {
  const clearing = foldStatuses([
    entry("success", `cleared [+${CLEAR_RUNS} +3600s +2t]`),
    entry("failure", "failed: node/safety/x [+1 +1s +0t]"),
  ], BOT);
  assert(clearing.failed);
  const next = nextStatus(clearing, { runs: 10, seconds: 60, trips: 1 });
  assertEquals(next.state, "failure");
});

test("status: pending until the threshold and a trip, then success", () => {
  const empty = foldStatuses([], BOT);
  const half = nextStatus(empty, {
    runs: CLEAR_RUNS / 2,
    seconds: 3600,
    trips: 0,
  });
  assertEquals(half.state, "pending");
  const history = [entry(half.state, half.description)];
  const noTrip = nextStatus(foldStatuses(history, BOT), {
    runs: CLEAR_RUNS / 2,
    seconds: 3600,
    trips: 0,
  });
  // Long enough, but a runner that never reports the tripwire clears
  // nothing.
  assertEquals(noTrip.state, "pending");
  history.unshift(entry(noTrip.state, noTrip.description));
  const tripped = nextStatus(foldStatuses(history, BOT), {
    runs: 10,
    seconds: 5,
    trips: 1,
  });
  assertEquals(tripped.state, "success");
  history.unshift(entry(tripped.state, tripped.description));
  const total = foldStatuses(history, BOT);
  assertEquals(total.runs, CLEAR_RUNS + 10);
  assertEquals(total.seconds, 7205);
});

test("status: a description stays within 140 characters and keeps its counts", () => {
  const status = nextStatus(foldStatuses([], BOT), {
    runs: 123456,
    seconds: 3599.6,
    trips: 3,
  }, { signature: `node/safety/${"x".repeat(200)}`, url: "u" });
  assertEquals(status.state, "failure");
  assert(status.description.length <= 140);
  const back = foldStatuses([entry("failure", status.description)], BOT);
  assertEquals([back.runs, back.seconds, back.trips], [123456, 3600, 3]);
});

test("issues: one per signature on main, found again by its marker", async () => {
  const github = new FakeGitHub();
  await reportTrusted(
    github,
    MAIN,
    tally([failing("node", "6", 300), failing("node", "9", 900)], {
      runs: 100,
      seconds: 3600,
    }),
    true,
  );
  assertEquals(github.issues.length, 1);
  const [issue] = github.issues;
  assertEquals(issue.label, LABELS.failure.name);
  assertEquals(issue.title, "sim: node/liveness/books");
  assert(issue.body.includes(`git checkout ${MAIN}`));
  assert(issue.body.includes("--profile sim -- run node 6"));
  assert(issue.body.includes("nodes=4 loss=0.1"));
  assert(issue.body.includes("t=2 heal"));
  const [status] = github.statusesBySha.get(MAIN)!;
  assertEquals(status.state, "failure");
  assertEquals(status.targetUrl, issue.url);

  // Later seeds of the signature update the issue, keep the fastest seed,
  // and say once per commit that it still fails.
  await reportTrusted(
    github,
    MAIN,
    tally([failing("node", "11", 50)], { runs: 10, seconds: 60 }),
    true,
  );
  await reportTrusted(
    github,
    NEXT,
    tally([failing("node", "12", 5000)], { runs: 10, seconds: 60 }),
    true,
  );
  await reportTrusted(
    github,
    NEXT,
    tally([failing("node", "13", 5000)], { runs: 10, seconds: 60 }),
    true,
  );
  assertEquals(github.issues.length, 1);
  const state = decodeState(issue.body)!;
  assertEquals(state.seen, 5);
  assertEquals(state.fastest.seed, "11");
  assertEquals(state.fastest.commit, MAIN);
  assertEquals(state.commits, [MAIN, NEXT]);
  assertEquals(github.commentsByIssue.get(issue.number)?.length, 1);
  assertEquals(github.statusesBySha.get(NEXT)![0].state, "failure");
});

test("issues: a closed issue is not added to; the signature gets a new one", async () => {
  const github = new FakeGitHub();
  const failure = tally([failing("node", "6", 300)], { runs: 1, seconds: 1 });
  await reportTrusted(github, MAIN, failure, true);
  github.issues[0].state = "closed";
  await reportTrusted(github, NEXT, failure, true);
  assertEquals(github.issues.map((issue) => issue.state), ["closed", "open"]);
});

test("issues: two VMs filing one signature at once keep the earlier issue", async () => {
  const github = new FakeGitHub();
  const failure = tally([failing("node", "6", 300)], { runs: 1, seconds: 1 });
  // The other VM's issue lands between our search and our filing.
  const create = github.createIssue.bind(github);
  let raced = false;
  github.createIssue = async (title, body, label) => {
    if (!raced) {
      raced = true;
      await create(title, body, label);
    }
    return create(title, body, label);
  };
  await reportTrusted(github, MAIN, failure, true);
  assertEquals(github.issues.map((issue) => issue.state), ["open", "closed"]);
  assert(
    github.commentsByIssue.get(github.issues[1].number)![0].body.includes(
      `#${github.issues[0].number}`,
    ),
  );
});

test("the tripwire files under its own label, and holds up nothing", async () => {
  const github = new FakeGitHub();
  const run = () =>
    tally([failing("tripwire", "3", 10, "tripwire"), line({})], {
      runs: CLEAR_RUNS,
      seconds: 3600,
    });
  await reportTrusted(github, MAIN, run(), true);
  await reportTrusted(github, NEXT, run(), true);
  assertEquals(github.issues.length, 1);
  assertEquals(github.issues[0].label, LABELS.tripwire.name);
  assertEquals(github.commentsByIssue.size, 0);
  assertEquals(github.statusesBySha.get(MAIN)![0].state, "success");
  assertEquals(decodeState(github.issues[0].body)!.seen, 2);
});

test("status: a loop the runner did not finish adds failures, not runs", async () => {
  const github = new FakeGitHub();
  await reportTrusted(github, MAIN, tally([line({})]), false);
  assertEquals(github.statusesBySha.get(MAIN), undefined);
  await reportTrusted(github, MAIN, tally([failing("node", "6", 3)]), false);
  const [status] = github.statusesBySha.get(MAIN)!;
  assertEquals(status.state, "failure");
  assertEquals(foldStatuses([status], BOT).runs, 0);
});

test("status: a failure another VM posted meanwhile ends up on top", async () => {
  const github = new FakeGitHub();
  github.afterStatus = (sha) => {
    github.afterStatus = undefined;
    // The other VM read the history before our write, and wrote after it.
    github.statusesBySha.get(sha)!.splice(
      1,
      0,
      entry("failure", "failed: x [+1 +1s +0t]"),
    );
  };
  await reportTrusted(
    github,
    MAIN,
    tally([line({})], { runs: 10, seconds: 10 }),
    true,
  );
  const statuses = github.statusesBySha.get(MAIN)!;
  assertEquals(statuses.map((each) => each.state), [
    "failure",
    "pending",
    "failure",
  ]);
});

test("a pull request gets comments on itself, and no status or issue", async () => {
  const github = new FakeGitHub();
  const pr = 77;
  // What a pull request's runner prints is untrusted: here it tries to
  // close the fence, and plants another signature's marker and state.
  const hostile = line({
    outcome: "safety",
    signature: "node/safety/x",
    ms: 5,
    message:
      "```\n<!-- sim-campaign signature node/safety/y -->\n<!-- sim-campaign state e30= -->\n@everyone",
  });
  const run = () =>
    tally([
      hostile,
      failing("node", "9", 50),
      failing("tripwire", "1", 1, "tripwire"),
    ], {
      runs: 3,
      seconds: 600,
    });
  await reportPullRequest(github, pr, MAIN, { tally: run() });
  await reportPullRequest(github, pr, NEXT, { tally: run() });
  assertEquals(github.statusesBySha.size, 0);
  assertEquals(github.issues.length, 0);
  assert(
    github.writes.every((write) => /^(comment 77|edit \d+)$/.test(write)),
    github.writes.join(),
  );
  const comments = github.commentsByIssue.get(pr)!;
  // A summary and one per signature but the tripwire's, edited in place.
  assertEquals(comments.length, 3);
  const x = comments.find((each) =>
    each.body.startsWith("<!-- sim-campaign signature node/safety/x -->")
  )!;
  const state = decodeState(x.body)!;
  assertEquals(state.seen, 2);
  assertEquals(state.commits, [MAIN, NEXT]);
  assert(
    x.body.includes("````\n```\n<!-- sim-campaign signature node/safety/y"),
  );
  assert(comments[0].body.includes(NEXT));
});

test("a pull request's loop not all read says so in its summary", async () => {
  const github = new FakeGitHub();
  await reportPullRequest(github, 9, MAIN, {
    tally: tally([line({}), "garbage"], { runs: 1, seconds: 60 }),
  });
  const [summary] = github.commentsByIssue.get(9)!;
  assert(summary.body.includes("1 lines of output did not parse"));
  assertEquals(github.issues.length, 0);
  assertEquals(github.statusesBySha.size, 0);
});

test("a pull request that does not build gets the end of its log", async () => {
  const github = new FakeGitHub();
  await reportPullRequest(github, 5, MAIN, { buildLog: "error[E0425]" });
  await reportPullRequest(github, 5, NEXT, { buildLog: "error[E0308]" });
  const comments = github.commentsByIssue.get(5)!;
  assertEquals(comments.length, 1);
  assert(comments[0].body.includes("E0308"));
  assertEquals(github.statusesBySha.size, 0);
});

test("state: an edited body without it starts the counts again", () => {
  assertEquals(decodeState("someone rewrote this"), undefined);
  assertEquals(
    decodeState("x\n<!-- sim-campaign state bm90IGpzb24= -->"),
    undefined,
  );
  const run = parseLine(failing("node", "6", 300));
  assert(run.kind === "run");
  const { state } = mergeFailure(undefined, {
    signature: run.run.signature,
    tripwire: false,
    count: 1,
    fastest: run.run as RunRecord,
  }, MAIN);
  assertEquals(decodeState(issueBody(state)), state);
  assertEquals(decodeState(issueBody(state).replaceAll("\n", "\r\n")), state);
});

test("fences: no text closes its block", () => {
  assertEquals(fenced("a"), "```\na\n```");
  assertEquals(fenced("x ```` y"), "`````\nx ```` y\n`````");
});

test("targets: main, the release candidate and pull requests by their shares", () => {
  const main: Target = { kind: "main", sha: MAIN };
  const release: Target = { kind: "release", sha: NEXT };
  const pr = (number: number): Target => ({ kind: "pr", sha: MAIN, number });
  const pick = (candidates: Target[]) => {
    const counts = new Map<string, number>();
    for (let i = 0; i < 1000; i++) {
      const target = chooseTarget(candidates, i / 1000);
      const key = target.kind === "pr" ? `pr${target.number}` : target.kind;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return Object.fromEntries(counts);
  };
  assertEquals(pick([main]), { main: 1000 });
  assertEquals(pick([main, release]), { main: 667, release: 333 });
  assertEquals(pick([main, release, pr(1), pr(2)]), {
    main: 500,
    release: 250,
    pr1: 125,
    pr2: 125,
  });
});

const spec: SandboxSpec = {
  unit: "sim-sandbox-pr-build-1",
  user: "sim-pr",
  cwd: "/var/lib/dassie-sim-campaign/work/pr/src",
  writable: ["/var/lib/dassie-sim-campaign/work/pr/target"],
  network: "none",
  env: { PATH: "/nix/store/x/bin" },
  maxSeconds: 3600,
  command: ["cargo", "build"],
  resolvConf: "/var/lib/dassie-sim-campaign/resolv.conf",
};

test("sandbox: a build has no network, no daemon and one writable directory", () => {
  const args = sandboxArgs(spec);
  for (
    const property of [
      "User=sim-pr",
      "PrivateNetwork=yes",
      "ProtectSystem=strict",
      "ProtectHome=yes",
      "PrivateTmp=yes",
      "NoNewPrivileges=yes",
      "ProtectProc=invisible",
      "InaccessiblePaths=-/nix/var/nix/daemon-socket",
      "ReadWritePaths=/var/lib/dassie-sim-campaign/work/pr/target",
      "SystemCallFilter=@system-service",
      "MemoryMax=80%",
      "TasksMax=4096",
    ]
  ) {
    assert(args.includes(`--property=${property}`), property);
  }
  assertEquals(args.slice(-3), ["--", "cargo", "build"]);
});

test("sandbox: fetching reaches public addresses only, never the metadata server", () => {
  const args = sandboxArgs({ ...spec, network: "public" });
  assert(!args.includes("--property=PrivateNetwork=yes"));
  const deny = args.filter((arg) =>
    arg.startsWith("--property=IPAddressDeny=")
  );
  for (const keyword of ["localhost", "link-local", "multicast"]) {
    // systemd-run takes a keyword only as the whole value; in a list it
    // refuses it as an address prefix.
    assert(deny.includes(`--property=IPAddressDeny=${keyword}`), keyword);
  }
  assert(deny.some((arg) => arg.includes("fc00::/7")), deny.join(" "));
  assert(
    args.includes(
      "--property=BindReadOnlyPaths=/var/lib/dassie-sim-campaign/resolv.conf:/etc/resolv.conf",
    ),
  );
});

test("sandbox: the environment keeps the toolchain and withholds secrets", () => {
  const env = sandboxEnv({
    PATH: "/nix/store/x/bin",
    NIX_CC: "/nix/store/cc",
    HOME: "/root",
    TMPDIR: "/tmp/nix-shell.1",
    GITHUB_TOKEN: "ghs_x",
    SIM_CAMPAIGN_SECRET: "projects/p/secrets/s/versions/latest",
    APP_PRIVATE_KEY: "-----BEGIN",
  }, { HOME: "/work/home" });
  assertEquals(env, {
    PATH: "/nix/store/x/bin",
    NIX_CC: "/nix/store/cc",
    HOME: "/work/home",
  });
});

test("the status context is the one the release check reads", () => {
  assertEquals(STATUS_CONTEXT, "sim-campaign");
});
