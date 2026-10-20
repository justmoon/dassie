// Whether the simulation campaign has cleared a commit for release
// (spec D27, spec/areas/release.md#publishing):
//
//   node scripts/campaign-status.ts [<commit>] [--tag <tag> --actor <login>]
//
// The campaign's supervisor marks each commit it runs with a commit status
// of the context `sim-campaign`: pending while it has run the commit for
// less than it must, success once it has run it long enough without a
// failure, and failure, for good, once it saw one. A commit is cleared when
// the newest of these statuses is success and none was a failure.
//
// The release workflow runs this before it builds anything, and again
// before it signs, with the tag and who pushed it, so that its refusal says
// what to do with the tag. The owner runs it before tagging (the commit
// defaults to HEAD), so that a tag the workflow would refuse is never
// pushed. It exits 0 when the commit is cleared, 1 when it is not, and 2
// when it cannot tell.
//
// The repository is $GITHUB_REPOSITORY, by default justmoon/dassie. A token
// in $GITHUB_TOKEN or $GH_TOKEN is sent if there is one; the repository is
// public, so the owner needs none.

import { execFile } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

/** The context of the campaign's statuses. */
export const CONTEXT = "sim-campaign";

/**
 * The one account whose statuses count, compared without case, as GitHub
 * compares logins; a GitHub App posts as `<slug>[bot]`. Anyone who can
 * push to the repository, and any workflow with `statuses: write`, can
 * post a status of the campaign's context on any commit, so the context
 * alone proves nothing; only the campaign holds this account's token.
 * It is the campaign's GitHub App. Set to null, no commit is cleared and
 * every release is refused.
 */
export const CAMPAIGN_ACCOUNT: string | null = "dassie-sim-campaign[bot]";

/** GitHub keeps at most 1000 statuses per commit and context, 100 to a
 * page. */
const PAGE_SIZE = 100;
const MAX_PAGES = 10;

/** A commit status, as the REST API lists it; only what is read here. */
export interface Status {
  id: number;
  state: string;
  context: string;
  description: string | null;
  target_url: string | null;
  created_at: string;
  creator: { login: string } | null;
}

export type Verdict =
  | { kind: "cleared" | "failed" | "pending"; status: Status; ignored: number }
  | { kind: "unseen"; ignored: number }
  | { kind: "untrusted" };

/** The status created last; GitHub's ids grow with time, and break a tie
 * within the same second. */
function newest(statuses: Status[]): Status | undefined {
  let best: Status | undefined;
  let bestTime = 0;
  for (const status of statuses) {
    const time = Date.parse(status.created_at);
    if (Number.isNaN(time)) {
      throw new Error(
        `status ${status.id} has no time: ${JSON.stringify(status.created_at)}`,
      );
    }
    if (
      best === undefined || time > bestTime ||
      (time === bestTime && status.id > best.id)
    ) {
      best = status;
      bestTime = time;
    }
  }
  return best;
}

/** What the campaign's own statuses on a commit say about it. A failure
 * is final, whatever came after it: the supervisor never clears a commit it
 * saw fail, and nor does this. */
export function judge(
  statuses: Status[],
  account: string | null = CAMPAIGN_ACCOUNT,
): Verdict {
  if (account === null) return { kind: "untrusted" };
  const login = account.toLowerCase();
  const context = statuses.filter((status) => status.context === CONTEXT);
  const own = context.filter((status) =>
    status.creator?.login.toLowerCase() === login
  );
  const ignored = context.length - own.length;
  for (const status of own) {
    if (!["pending", "success", "failure", "error"].includes(status.state)) {
      throw new Error(`status ${status.id} has state ${status.state}`);
    }
  }
  const failure = newest(
    own.filter((status) =>
      status.state === "failure" || status.state === "error"
    ),
  );
  if (failure !== undefined) {
    return { kind: "failed", status: failure, ignored };
  }
  const latest = newest(own);
  if (latest === undefined) return { kind: "unseen", ignored };
  return {
    kind: latest.state === "success" ? "cleared" : "pending",
    status: latest,
    ignored,
  };
}

/** The tag a release workflow run refuses, and who pushed it. */
export interface Tag {
  name: string;
  actor: string;
}

function said(status: Status): string {
  const parts = [status.description, status.target_url].filter(Boolean);
  return parts.length > 0 ? ` (${parts.join(", ")})` : "";
}

/** What a verdict means, and what to do about it, in plain lines. */
export function explain(verdict: Verdict, commit: string, tag?: Tag): string {
  const check = `\`node scripts/campaign-status.ts <commit>\``;
  const deleteTag = tag &&
    `${tag.actor}, who pushed ${tag.name}, deletes it, since this workflow cannot:\`git push origin :refs/tags/${tag.name}\`. Then tag the version again on a commit the campaign has cleared (${check} says which).`;
  const rerun = tag &&
    `The tag can stay: once the commit is cleared, re-run this workflow run, which checks again.`;
  switch (verdict.kind) {
    case "cleared":
      return `The simulation campaign cleared ${commit}${
        said(verdict.status)
      }.`;
    case "failed":
      return [
        `The simulation campaign saw a failure on ${commit}${
          said(verdict.status)
        }.`,
        "It never clears this commit, so no release is made from it. Fix the failure (its issue is labelled sim-failure), and release from a later commit once the campaign has cleared it.",
        deleteTag,
      ].filter(Boolean).join("\n");
    case "pending":
      return [
        `The simulation campaign has not yet run ${commit} long enough to clear it${
          said(verdict.status)
        }.`,
        "Wait until its sim-campaign status is success. The campaign runs the head of main: once main has moved on, it may never finish this commit, and then a later commit is the one to release.",
        rerun,
        tag && `If the campaign moves on instead: ${deleteTag}`,
      ].filter(Boolean).join("\n");
    case "unseen":
      return [
        `The simulation campaign has not run ${commit}.`,
        verdict.ignored > 0 &&
        `${verdict.ignored} status(es) of the context ${CONTEXT} from other accounts were ignored: only the campaign's own account counts.`,
        "The campaign runs the head of main, and pull requests that ask for it (spec D27); a commit main has moved past may never be run. Release from a commit it has cleared.",
        tag &&
        `If ${commit} is the head of main, the campaign will reach it: ${rerun}`,
        tag && `Otherwise: ${deleteTag}`,
      ].filter(Boolean).join("\n");
    case "untrusted":
      return [
        "No account is trusted to report for the simulation campaign: CAMPAIGN_ACCOUNT in scripts/campaign-status.ts names none, so no commit is cleared.",
        tag &&
        `Name the campaign's account there, in a reviewed pull request, before releasing. ${deleteTag}`,
      ].filter(Boolean).join("\n");
  }
}

/** Every status on `commit`, page by page. A commit that GitHub does not
 * have lists none. */
export async function fetchStatuses(
  repository: string,
  commit: string,
  token?: string,
  get: typeof fetch = fetch,
): Promise<Status[]> {
  const headers: Record<string, string> = {
    accept: "application/vnd.github+json",
    "x-github-api-version": "2022-11-28",
  };
  if (token) headers.authorization = `Bearer ${token}`;
  const statuses: Status[] = [];
  for (let page = 1; page <= MAX_PAGES; page++) {
    const url =
      `https://api.github.com/repos/${repository}/commits/${commit}/statuses?per_page=${PAGE_SIZE}&page=${page}`;
    const response = await get(url, { headers });
    if (!response.ok) {
      throw new Error(
        `GitHub answered ${response.status} for the statuses of ${commit}: ${
          (await response.text()).slice(0, 200)
        }`,
      );
    }
    const listed = await response.json() as Status[];
    if (!Array.isArray(listed)) {
      throw new Error(`GitHub listed no statuses for ${commit}`);
    }
    statuses.push(...listed);
    if (!/rel="next"/.test(response.headers.get("link") ?? "")) {
      return statuses;
    }
  }
  throw new Error(
    `${commit} has more than ${
      MAX_PAGES * PAGE_SIZE
    } statuses: more than GitHub keeps`,
  );
}

/** The full hash of a commit the local repository names. */
function resolve(commit: string): Promise<string> {
  if (/^[0-9a-f]{40}$/.test(commit)) return Promise.resolve(commit);
  return new Promise((done, fail) => {
    execFile(
      "git",
      ["rev-parse", "--verify", "--end-of-options", `${commit}^{commit}`],
      (error, stdout) => {
        if (error) fail(new Error(`git names no commit ${commit}`));
        else done(stdout.trim());
      },
    );
  });
}

/** One annotation in the workflow's log; its text cannot hold newlines. */
function annotate(level: "error" | "notice", text: string): string {
  const escaped = text.replaceAll("%", "%25").replaceAll("\r", "%0D")
    .replaceAll("\n", "%0A");
  return `::${level} title=Simulation campaign::${escaped}`;
}

const USAGE =
  "usage: campaign-status.ts [<commit>] [--tag <tag> --actor <login>]";

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let args;
  try {
    args = parseArgs({
      options: { tag: { type: "string" }, actor: { type: "string" } },
      allowPositionals: true,
    });
  } catch (error) {
    console.error(`${(error as Error).message}\n${USAGE}`);
    process.exit(2);
  }
  const { values, positionals } = args;
  if (
    positionals.length > 1 ||
    (values.tag === undefined) !== (values.actor === undefined)
  ) {
    console.error(USAGE);
    process.exit(2);
  }
  const tag = values.tag !== undefined && values.actor !== undefined
    ? { name: values.tag, actor: values.actor }
    : undefined;
  let verdict: Verdict;
  let commit: string;
  try {
    commit = await resolve(positionals[0] ?? "HEAD");
    verdict = judge(
      await fetchStatuses(
        process.env.GITHUB_REPOSITORY || "justmoon/dassie",
        commit,
        process.env.GITHUB_TOKEN || process.env.GH_TOKEN || undefined,
      ),
    );
  } catch (error) {
    console.error(
      `Cannot tell whether the simulation campaign cleared the commit: ${
        (error as Error).message
      }`,
    );
    process.exit(2);
  }
  const text = explain(verdict, commit, tag);
  const cleared = verdict.kind === "cleared";
  if (process.env.GITHUB_ACTIONS === "true") {
    console.log(annotate(cleared ? "notice" : "error", text));
  } else {
    (cleared ? console.log : console.error)(text);
  }
  process.exit(cleared ? 0 : 1);
}
