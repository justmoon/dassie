// scripts/campaign-status.ts against statuses as GitHub lists them, from
// fixtures: nothing here reaches the network.

import { test } from "node:test";
import {
  assert,
  assertEquals,
  assertMatch,
  assertRejects,
  assertThrows,
} from "@std/assert";
import {
  CONTEXT,
  explain,
  fetchStatuses,
  judge,
  type Status,
} from "./campaign-status.ts";

const ACCOUNT = "dassie-campaign[bot]";
const COMMIT = "1efa014029902b936f6172281adf6f43a12e274e";

let nextId = 1;

/** A status as the REST API lists it, `second` seconds into a day. */
function status(
  state: string,
  second: number,
  overrides: Partial<Status> = {},
): Status {
  return {
    id: nextId++,
    state,
    context: CONTEXT,
    description: `${state} after ${second}s`,
    target_url: null,
    created_at: new Date(Date.UTC(2026, 9, 20, 0, 0, second)).toISOString()
      .replace(".000", ""),
    creator: { login: ACCOUNT },
    ...overrides,
  };
}

test("a commit whose newest status is success is cleared", () => {
  const verdict = judge(
    [status("success", 30), status("pending", 10), status("pending", 20)],
    ACCOUNT,
  );
  assertEquals(verdict.kind, "cleared");
  assert(verdict.kind === "cleared");
  assertEquals(verdict.status.description, "success after 30s");
});

test("a commit still running is pending", () => {
  const verdict = judge(
    [status("pending", 10), status("pending", 20)],
    ACCOUNT,
  );
  assert(verdict.kind === "pending");
  assertEquals(verdict.status.description, "pending after 20s");
});

test("the order GitHub lists them in does not matter", () => {
  assertEquals(
    judge([status("pending", 10), status("success", 30)], ACCOUNT).kind,
    "cleared",
  );
  assertEquals(
    judge([status("success", 10), status("pending", 30)], ACCOUNT).kind,
    "pending",
  );
});

test("a failure is final, even under a later success", () => {
  const verdict = judge(
    [status("success", 30), status("failure", 20), status("pending", 10)],
    ACCOUNT,
  );
  assert(verdict.kind === "failed");
  assertEquals(verdict.status.description, "failure after 20s");
  assertEquals(judge([status("error", 5)], ACCOUNT).kind, "failed");
});

test("only the campaign's account counts", () => {
  const forged = status("success", 40, {
    creator: { login: "github-actions[bot]" },
  });
  const verdict = judge([forged, status("pending", 10)], ACCOUNT);
  assert(verdict.kind === "pending");
  assertEquals(verdict.ignored, 1);
  assertEquals(judge([forged], ACCOUNT), { kind: "unseen", ignored: 1 });
  // GitHub's logins are not case sensitive.
  assertEquals(
    judge([
      status("success", 1, { creator: { login: "Dassie-Campaign[bot]" } }),
    ], ACCOUNT)
      .kind,
    "cleared",
  );
  // A deleted account lists no creator.
  assertEquals(
    judge([status("success", 1, { creator: null })], ACCOUNT).kind,
    "unseen",
  );
});

test("other contexts do not count", () => {
  assertEquals(
    judge([status("success", 1, { context: "Vercel" })], ACCOUNT),
    { kind: "unseen", ignored: 0 },
  );
  assertEquals(judge([], ACCOUNT), { kind: "unseen", ignored: 0 });
});

test("with no account trusted, nothing is cleared", () => {
  assertEquals(judge([status("success", 1)], null), { kind: "untrusted" });
});

test("ties in time go to the later status", () => {
  const pending = status("pending", 10);
  const success = status("success", 10);
  assertEquals(judge([success, pending], ACCOUNT).kind, "cleared");
});

test("refuses a state it does not know, and a status with no time", () => {
  assertThrows(
    () => judge([status("skipped", 1)], ACCOUNT),
    Error,
    "has state skipped",
  );
  assertThrows(
    () => judge([status("success", 1, { created_at: "yesterday" })], ACCOUNT),
    Error,
    "has no time",
  );
});

test("says what to do with a refused tag", () => {
  const tag = { name: "v0.2.0", actor: "justmoon" };
  const failed = explain(judge([status("failure", 1)], ACCOUNT), COMMIT, tag);
  assertMatch(failed, /saw a failure on 1efa0140/);
  assertMatch(failed, /justmoon, who pushed v0\.2\.0, deletes it/);
  assertMatch(failed, /git push origin :refs\/tags\/v0\.2\.0/);
  const pending = explain(judge([status("pending", 1)], ACCOUNT), COMMIT, tag);
  assertMatch(pending, /not yet run 1efa0140\w* long enough/);
  assertMatch(pending, /re-run this workflow run/);
  const unseen = explain(judge([], ACCOUNT), COMMIT, tag);
  assertMatch(unseen, /has not run 1efa0140/);
  assertMatch(unseen, /Otherwise: justmoon, who pushed/);
  // Without a tag, as the owner runs it before tagging, there is no tag to
  // speak of.
  assert(!explain(judge([], ACCOUNT), COMMIT).includes("tag"));
  assertMatch(
    explain(judge([status("success", 1)], ACCOUNT), COMMIT),
    /^The simulation campaign cleared 1efa0140\w* \(success after 1s\)\.$/,
  );
  assertMatch(
    explain({ kind: "untrusted" }, COMMIT, tag),
    /CAMPAIGN_ACCOUNT in scripts\/campaign-status\.ts names none/,
  );
});

/** A fetch that answers from `pages`, and records what it was asked. */
function fixture(pages: { body: unknown; status?: number; last?: boolean }[]) {
  const asked: { url: string; authorization?: string }[] = [];
  const get = ((url: string, init?: RequestInit) => {
    const headers = init?.headers as Record<string, string>;
    asked.push({ url, authorization: headers.authorization });
    const page = pages[asked.length - 1];
    assert(page !== undefined, `asked for page ${asked.length}`);
    const link = page.last
      ? ""
      : `<https://api.github.com/x?page=${asked.length + 1}>; rel="next"`;
    return Promise.resolve(
      new Response(JSON.stringify(page.body), {
        status: page.status ?? 200,
        headers: link ? { link } : {},
      }),
    );
  }) as typeof fetch;
  return { get, asked };
}

test("reads every page of statuses", async () => {
  const { get, asked } = fixture([
    { body: [status("pending", 1)] },
    { body: [status("success", 2)], last: true },
  ]);
  const statuses = await fetchStatuses("justmoon/dassie", COMMIT, "t0ken", get);
  assertEquals(statuses.length, 2);
  assertEquals(asked.length, 2);
  assertMatch(
    asked[1].url,
    /^https:\/\/api\.github\.com\/repos\/justmoon\/dassie\/commits\/1efa0140\w+\/statuses\?per_page=100&page=2$/,
  );
  assertEquals(asked[0].authorization, "Bearer t0ken");
});

test("sends no token when it has none", async () => {
  const { get, asked } = fixture([{ body: [], last: true }]);
  assertEquals(
    await fetchStatuses("justmoon/dassie", COMMIT, undefined, get),
    [],
  );
  assertEquals(asked[0].authorization, undefined);
});

test("stops after as many statuses as GitHub keeps", async () => {
  const { get, asked } = fixture(
    Array.from({ length: 10 }, () => ({ body: [] })),
  );
  await assertRejects(
    () => fetchStatuses("justmoon/dassie", COMMIT, undefined, get),
    Error,
    "more than 1000 statuses",
  );
  assertEquals(asked.length, 10);
});

test("an error from GitHub is no verdict", async () => {
  const { get } = fixture([{
    body: { message: "Bad credentials" },
    status: 401,
  }]);
  await assertRejects(
    () => fetchStatuses("justmoon/dassie", COMMIT, "bad", get),
    Error,
    "GitHub answered 401",
  );
});
