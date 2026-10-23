// scripts/sim-campaign.ts's logic against fixtures and a GitHub in memory:
// no network, no sandbox.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import {
  type CampaignResult,
  chooseTarget,
  cleanSignature,
  type Comment,
  decodeState,
  decodeTrouble,
  fenced,
  type GitHub,
  type Issue,
  issueBody,
  LABELS,
  mergeFailure,
  mergeTrouble,
  parseLine,
  reportMain,
  reportPullRequest,
  reportTrouble,
  type RunRecord,
  sandboxArgs,
  sandboxEnv,
  type SandboxSpec,
  Tally,
  type Target,
  TROUBLE,
  type Trouble,
  troubleBody,
  troubleOf,
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
  issues:
    (Issue & { label: string; state: "open" | "closed"; title: string })[] = [];
  commentsByIssue = new Map<number, Comment[]>();
  writes: string[] = [];
  #next = 100;

  ensureLabel(label: { name: string }) {
    this.writes.push(`label ${label.name}`);
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

test("a loop's tally: runs, failures by signature, the fastest seed", () => {
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

test("main: a loop not all read fails closed", async () => {
  const github = new FakeGitHub();
  // Only the tripwire's failures parsed; a real one was lost in a line too
  // long to read.
  const loop = tally([failing("tripwire", "3", 10, "tripwire"), "{not json"]);
  loop.add(
    parseLine('{"summary":{"runs":2,"by_sim":{},"failures":2,"seconds":3600}}'),
  );
  await reportMain(github, MAIN, loop, true);
  const issue = github.issues.find((each) =>
    each.label === LABELS.failure.name
  )!;
  assertEquals(issue.title, "sim: campaign/unreadable");
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

test("main: an unfinished loop's last line, cut short, fails nothing", async () => {
  const github = new FakeGitHub();
  await reportMain(github, MAIN, cutShort([line({})], CUT), false);
  assertEquals(github.issues.length, 0);
  // Its real failures still count.
  await reportMain(
    github,
    MAIN,
    cutShort([failing("node", "5", 10)], CUT),
    false,
  );
  assertEquals(github.issues.map((each) => each.title), [
    "sim: node/liveness/books",
  ]);
});

test("main: an unfinished loop with a line it could not read before its cut last line fails closed", async () => {
  const github = new FakeGitHub();
  // A fully written line that did not parse may be a failure: the kill
  // that cut the last line does not explain it.
  await reportMain(
    github,
    MAIN,
    cutShort([line({}), "{not json"], CUT),
    false,
  );
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

test("issues: one per signature on main, found again by its marker", async () => {
  const github = new FakeGitHub();
  await reportMain(
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

  // Later seeds of the signature update the issue, keep the fastest seed,
  // and say once per commit that it still fails.
  await reportMain(
    github,
    MAIN,
    tally([failing("node", "11", 50)], { runs: 10, seconds: 60 }),
    true,
  );
  await reportMain(
    github,
    NEXT,
    tally([failing("node", "12", 5000)], { runs: 10, seconds: 60 }),
    true,
  );
  await reportMain(
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
});

test("issues: a closed issue is not added to; the signature gets a new one", async () => {
  const github = new FakeGitHub();
  const failure = tally([failing("node", "6", 300)], { runs: 1, seconds: 1 });
  await reportMain(github, MAIN, failure, true);
  github.issues[0].state = "closed";
  await reportMain(github, NEXT, failure, true);
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
  await reportMain(github, MAIN, failure, true);
  assertEquals(github.issues.map((issue) => issue.state), ["open", "closed"]);
  assert(
    github.commentsByIssue.get(github.issues[1].number)![0].body.includes(
      `#${github.issues[0].number}`,
    ),
  );
});

test("the tripwire files under its own label, and is never commented on", async () => {
  const github = new FakeGitHub();
  const run = () =>
    tally([failing("tripwire", "3", 10, "tripwire"), line({})], {
      runs: 100,
      seconds: 3600,
    });
  await reportMain(github, MAIN, run(), true);
  await reportMain(github, NEXT, run(), true);
  assertEquals(github.issues.length, 1);
  assertEquals(github.issues[0].label, LABELS.tripwire.name);
  assertEquals(github.commentsByIssue.size, 0);
  assertEquals(decodeState(github.issues[0].body)!.seen, 2);
});

test("main: a loop files issues and nothing else", async () => {
  const github = new FakeGitHub();
  await reportMain(
    github,
    MAIN,
    tally([line({})], { runs: 10, seconds: 60 }),
    true,
  );
  assertEquals(github.writes, []);
  await reportMain(github, MAIN, tally([failing("node", "6", 3)]), false);
  assert(
    github.writes.every((write) => /^issue \d+ sim-failure$/.test(write)),
    github.writes.join(),
  );
});

test("a pull request gets comments on itself, and no issue", async () => {
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
});

test("a pull request that does not build gets the end of its log", async () => {
  const github = new FakeGitHub();
  await reportPullRequest(github, 5, MAIN, { buildLog: "error[E0425]" });
  await reportPullRequest(github, 5, NEXT, { buildLog: "error[E0308]" });
  const comments = github.commentsByIssue.get(5)!;
  assertEquals(comments.length, 1);
  assert(comments[0].body.includes("E0308"));
});

function trouble(fields: Partial<Trouble> = {}): Trouble {
  return {
    sha: MAIN,
    step: "build",
    log: "error: linker `cc` not found",
    time: "2026-10-20T10:00:00.000Z",
    host: "sim-campaign-1",
    ...fields,
  };
}

test("trouble: a commit of main that does not build files one issue of its own", async () => {
  const github = new FakeGitHub();
  await reportTrouble(github, trouble());
  await reportTrouble(
    github,
    trouble({ time: "2026-10-20T10:01:00.000Z", log: "error: disk full" }),
  );
  await reportTrouble(
    github,
    trouble({
      sha: NEXT,
      step: "fetch",
      time: "2026-10-20T10:02:00.000Z",
    }),
  );
  assertEquals(github.issues.length, 1);
  const [issue] = github.issues;
  assertEquals(issue.label, LABELS.trouble.name);
  assert(issue.label !== LABELS.failure.name);
  assert(issue.body.startsWith(`<!-- sim-campaign signature ${TROUBLE} -->`));
  // Updated in place, never commented on: the supervisor retries every
  // minute.
  assertEquals(github.commentsByIssue.size, 0);
  assert(
    github.writes.every((write) =>
      /^(issue \d+ sim-campaign|update \d+)$/.test(write)
    ),
    github.writes.join(),
  );
  const state = decodeTrouble(issue.body)!;
  assertEquals(state.seen, 3);
  assertEquals(state.first, "2026-10-20T10:00:00.000Z");
  assertEquals(state.latest.time, "2026-10-20T10:02:00.000Z");
  assertEquals(state.commits, [MAIN, NEXT]);
  assert(issue.body.includes("Seen 3 times since 2026-10-20T10:00:00.000Z"));
  assert(issue.body.includes(`fetching the dependencies of main ${NEXT}`));
  assert(issue.body.includes("sim-campaign-1"));
  assert(issue.body.includes("linker `cc` not found"));
});

test("trouble: the issue's log and commits stay bounded", async () => {
  const github = new FakeGitHub();
  for (let i = 0; i < 30; i++) {
    await reportTrouble(
      github,
      trouble({
        sha: i.toString(16).padStart(40, "0"),
        log: `start ${"x".repeat(100_000)} end ${i}`,
      }),
    );
  }
  const [issue] = github.issues;
  const state = decodeTrouble(issue.body)!;
  assertEquals(state.seen, 30);
  assertEquals(state.commits.length, 10);
  assertEquals(state.latest.log.length, 6_000);
  assert(state.latest.log.endsWith("end 29"));
  assert(issue.body.length < 20_000, String(issue.body.length));
});

test("trouble: a closed issue is not added to, and an edited body starts again", async () => {
  const github = new FakeGitHub();
  await reportTrouble(github, trouble());
  await reportTrouble(github, trouble());
  github.issues[0].state = "closed";
  await reportTrouble(github, trouble());
  assertEquals(github.issues.map((issue) => issue.state), ["closed", "open"]);
  assertEquals(decodeTrouble(github.issues[1].body)!.seen, 1);
  // Someone rewrote the body but kept its first line.
  github.issues[1].body = `<!-- sim-campaign signature ${TROUBLE} -->\nfixed?`;
  await reportTrouble(github, trouble());
  assertEquals(github.issues.length, 2);
  assertEquals(decodeTrouble(github.issues[1].body)!.seen, 1);
});

test("trouble: a build that failed, or a runner that failed before it printed a line", () => {
  assertEquals(troubleOf({ buildLog: "no space left", step: "fetch" }), {
    step: "fetch",
    log: "no space left",
  });
  const silent: CampaignResult = {
    tally: new Tally(),
    complete: false,
    code: 1,
    log: "Failed to start transient service unit",
  };
  assertEquals(troubleOf(silent), {
    step: "run",
    log: "Failed to start transient service unit",
  });
  // A runner that printed and then failed is the commit's to answer for.
  assertEquals(
    troubleOf({ ...silent, tally: tally([line({})]) }),
    undefined,
  );
  assertEquals(
    troubleOf({ ...silent, tally: tally(["garbage"]) }),
    undefined,
  );
  assertEquals(
    troubleOf({
      tally: tally([line({})], { runs: 1, seconds: 1 }),
      complete: true,
      code: 0,
      log: "",
    }),
    undefined,
  );
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
  // A failure's state is not the campaign's trouble, nor the other way.
  assertEquals(decodeTrouble(issueBody(state)), undefined);
  const trouble = mergeTrouble(undefined, {
    sha: MAIN,
    step: "build",
    log: "",
    time: "t",
    host: "h",
  });
  assertEquals(decodeTrouble(troubleBody(trouble)), trouble);
  assertEquals(decodeState(troubleBody(trouble)), undefined);
});

test("fences: no text closes its block", () => {
  assertEquals(fenced("a"), "```\na\n```");
  assertEquals(fenced("x ```` y"), "`````\nx ```` y\n`````");
});

test("targets: main and pull requests by their shares", () => {
  const main: Target = { kind: "main", sha: MAIN };
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
  assertEquals(pick([main, pr(1)]), { main: 667, pr1: 333 });
  assertEquals(pick([main, pr(1), pr(2)]), {
    main: 667,
    pr1: 167,
    pr2: 166,
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
