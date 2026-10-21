// Points https://get.dassie.land/flake/latest.tar.gz at a release's flake
// (spec/decisions/nix-flake-on-the-mirror.md), as the release workflow does
// once meta/latest names the release:
//
//   node scripts/flake-latest.ts <version> [--dry-run]
//
// Nix locks a tarball's URL by what it unpacked, so a URL whose contents
// change breaks every lock of it, unless the answer carries
// `Link: <url>; rel="immutable"`: Nix then locks that URL instead. A bucket
// cannot send that header, so the mirror's load balancer does: a route for
// /flake/latest.tar.gz rewrites it to /<version>/flake.tar.gz and adds the
// header, both in one change of its URL map, so the flake and the Link
// header always name the same version. Route rules' headers need the
// EXTERNAL_MANAGED scheme, which the mirror's load balancer has. Its
// backend for get.dassie.land has no Cloud CDN, which would keep the old
// version under the same URL.
//
// The route lives in a path matcher of its own for get.dassie.land, which
// this script makes the first time, with the service get.dassie.land has
// now. Every other entry of the map stays as it is
// (scripts/website/provision.sh keeps it too). The map is read, changed,
// validated for the load balancer's scheme, and updated through the Compute
// API with the fingerprint it was read with, so a change in between fails
// the update, and the script reads the map again. gcloud would do neither:
// `url-maps validate` exits 0 on a map that does not load, and
// `url-maps import` reads the map again itself and sends that fingerprint,
// overwriting a change made in between.
//
// It takes its access token from gcloud, signed in as an account that may
// read, validate and update the URL map (scripts/flake-latest/provision.sh).
// `--dry-run` prints the new map and changes nothing.

import { spawn } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const PROJECT = "dassie-390023";
export const URL_MAP = "get-dassie-land";
export const HOST = "get.dassie.land";
/** The path matcher this script owns. */
export const MATCHER = "get-dassie-land-flake";
export const PATH = "/flake/latest.tar.gz";
/** The mirror's load balancer's scheme, which the map is validated for. */
export const SCHEME = "EXTERNAL_MANAGED";

/** A URL map as the Compute API gives it: only the parts read here are
 * typed. */
export interface UrlMap {
  defaultService?: string;
  hostRules?: { hosts: string[]; pathMatcher: string }[];
  pathMatchers?: {
    name: string;
    defaultService?: string;
    routeRules?: RouteRule[];
    [key: string]: unknown;
  }[];
  [key: string]: unknown;
}

interface RouteRule {
  matchRules?: { fullPathMatch?: string; [key: string]: unknown }[];
  service?: string;
  routeAction?: { urlRewrite?: { pathPrefixRewrite?: string } };
  headerAction?: {
    responseHeadersToAdd?: {
      headerName: string;
      headerValue: string;
      replace?: boolean;
    }[];
  };
  [key: string]: unknown;
}

const RELEASE = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

/** `map` with /flake/latest.tar.gz on get.dassie.land routed to
 * `version`'s flake. */
export function routeFlake(map: UrlMap, version: string): UrlMap {
  if (!RELEASE.test(version)) {
    throw new Error(`${JSON.stringify(version)} is not a release version`);
  }
  const hostRules = map.hostRules ?? [];
  const pathMatchers = map.pathMatchers ?? [];
  for (const rule of hostRules) {
    for (const host of rule.hosts) {
      const covers = host === HOST || host === "*" ||
        (host.startsWith("*.") && HOST.endsWith(host.slice(1)));
      if (covers && rule.pathMatcher !== MATCHER) {
        throw new Error(
          `${host} already goes to the path matcher ${rule.pathMatcher}: route ${PATH} by hand`,
        );
      }
    }
  }
  const existing = pathMatchers.find((matcher) => matcher.name === MATCHER);
  // get.dassie.land keeps the service it has now: the map's default, the
  // first time. A default that is more than a service would not carry over.
  if (!existing && (map.defaultRouteAction || map.defaultUrlRedirect)) {
    throw new Error(
      `the URL map's default is more than a service: route ${PATH} by hand`,
    );
  }
  const service = existing?.defaultService ?? map.defaultService;
  if (!service) {
    throw new Error(`the URL map has no service for ${HOST}`);
  }
  const target = `https://${HOST}/${version}/flake.tar.gz`;
  const matcher = {
    name: MATCHER,
    defaultService: service,
    routeRules: [
      {
        priority: 1,
        matchRules: [{ fullPathMatch: PATH }],
        service,
        routeAction: {
          urlRewrite: { pathPrefixRewrite: `/${version}/flake.tar.gz` },
        },
        headerAction: {
          responseHeadersToAdd: [
            {
              headerName: "Link",
              headerValue: `<${target}>; rel="immutable"`,
              replace: true,
            },
            {
              headerName: "Cache-Control",
              headerValue: "no-cache, max-age=0",
              replace: true,
            },
          ],
        },
      },
    ],
  };
  const {
    id: _id,
    kind: _kind,
    selfLink: _selfLink,
    creationTimestamp: _created,
    ...rest
  } = map;
  const routed = hostRules.some((rule) =>
    rule.pathMatcher === MATCHER && rule.hosts.includes(HOST)
  );
  return {
    ...rest,
    hostRules: routed
      ? hostRules
      : [...hostRules, { hosts: [HOST], pathMatcher: MATCHER }],
    pathMatchers: existing
      ? pathMatchers.map((m) => m.name === MATCHER ? matcher : m)
      : [...pathMatchers, matcher],
  };
}

/** Whether `map` already routes the flake to `version`: get.dassie.land
 * goes to the matcher, whose one route rewrites to the version's flake, on
 * the matcher's own service, with the Link header naming it. The live
 * map's other fields, defaults and key order do not count. */
export function routes(map: UrlMap, version: string): boolean {
  const routed = map.hostRules?.some((rule) =>
    rule.pathMatcher === MATCHER && rule.hosts.includes(HOST)
  );
  const matcher = map.pathMatchers?.find((m) => m.name === MATCHER);
  const rules = matcher?.routeRules ?? [];
  if (!routed || !matcher || rules.length !== 1) return false;
  const [rule] = rules;
  const headers = rule.headerAction?.responseHeadersToAdd ?? [];
  const header = (name: string) =>
    headers.find((h) => h.headerName === name)?.headerValue;
  return rule.matchRules?.length === 1 &&
    rule.matchRules[0].fullPathMatch === PATH &&
    rule.service === matcher.defaultService &&
    rule.routeAction?.urlRewrite?.pathPrefixRewrite ===
      `/${version}/flake.tar.gz` &&
    header("Link") ===
      `<https://${HOST}/${version}/flake.tar.gz>; rel="immutable"` &&
    header("Cache-Control") === "no-cache, max-age=0";
}

/** What `gcloud` prints. */
function gcloud(args: string[]): Promise<string> {
  const child = spawn("gcloud", args, {
    stdio: ["ignore", "pipe", "inherit"],
  });
  let stdout = "";
  child.stdout.setEncoding("utf8").on("data", (text: string) => {
    stdout += text;
  });
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) =>
      code === 0 ? resolve(stdout) : reject(
        new Error(`gcloud ${args.slice(0, 2).join(" ")} failed (${code})`),
      ));
  });
}

const API =
  `https://compute.googleapis.com/compute/v1/projects/${PROJECT}/global`;

interface Answer {
  status: number;
  json: Record<string, unknown>;
}

/** A Compute API call with gcloud's access token. */
async function compute(
  token: string,
  method: string,
  path: string,
  body?: unknown,
): Promise<Answer> {
  const response = await fetch(`${API}/${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = await response.json() as Record<string, unknown>;
  return { status: response.status, json };
}

function failed(what: string, answer: Answer): Error {
  return new Error(`${what}: ${answer.status} ${JSON.stringify(answer.json)}`);
}

/** Waits, bounded, for a global operation to end, and fails if it did. */
async function finished(
  token: string,
  operation: Record<string, unknown>,
): Promise<void> {
  let current = operation;
  for (let polls = 0; current.status !== "DONE"; polls++) {
    if (polls >= 90) {
      throw new Error(`${String(operation.name)} did not end in 3 minutes`);
    }
    await new Promise((resolve) => setTimeout(resolve, 2000));
    const answer = await compute(
      token,
      "GET",
      `operations/${String(operation.name)}`,
    );
    if (answer.status !== 200) throw failed("reading the operation", answer);
    current = answer.json;
  }
  if (current.error) {
    throw new Error(`the update failed: ${JSON.stringify(current.error)}`);
  }
}

/** Reads the map, routes the flake, and validates and updates the map;
 * once more if the map changed in between. */
async function apply(version: string, dryRun: boolean): Promise<void> {
  const token = (await gcloud(["auth", "print-access-token"])).trim();
  for (let attempt = 1;; attempt++) {
    const read = await compute(token, "GET", `urlMaps/${URL_MAP}`);
    if (read.status !== 200) throw failed("reading the URL map", read);
    const map = read.json as UrlMap;
    if (routes(map, version)) {
      console.log(`${PATH} already names ${version}`);
      return;
    }
    const next = routeFlake(map, version);
    if (dryRun) {
      console.log(JSON.stringify(next, null, 2));
      return;
    }
    const check = await compute(
      token,
      "POST",
      `urlMaps/${URL_MAP}/validate`,
      { resource: next, loadBalancingSchemes: [SCHEME] },
    );
    const result = check.json.result as
      | { loadSucceeded?: boolean }
      | undefined;
    if (check.status !== 200 || result?.loadSucceeded !== true) {
      throw failed(`the new URL map does not load for ${SCHEME}`, check);
    }
    // `next` carries the fingerprint the map was read with: a change since
    // then is refused with 412.
    const update = await compute(token, "PUT", `urlMaps/${URL_MAP}`, next);
    if (update.status === 412 && attempt < 2) {
      console.error("the URL map changed since it was read; reading it again");
      continue;
    }
    if (update.status !== 200) throw failed("updating the URL map", update);
    await finished(token, update.json);
    console.log(`${PATH} names ${version}`);
    return;
  }
}

const USAGE = "usage: flake-latest.ts <version> [--dry-run]";

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let args;
  try {
    args = parseArgs({
      options: { "dry-run": { type: "boolean" } },
      allowPositionals: true,
    });
  } catch (error) {
    console.error(`${(error as Error).message}\n${USAGE}`);
    process.exit(2);
  }
  const [version] = args.positionals;
  if (!version || args.positionals.length !== 1) {
    console.error(USAGE);
    process.exit(2);
  }
  await apply(version, args.values["dry-run"] ?? false);
}
