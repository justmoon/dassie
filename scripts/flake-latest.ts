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
// header always name the same version. The route goes to a backend without
// Cloud CDN, which would keep the old version under the same URL.
//
// The route lives in a path matcher of its own for get.dassie.land, which
// this script makes the first time with the service get.dassie.land has
// now. Every other entry of the map stays as it is
// (scripts/website/provision.sh keeps it too). The map is read, changed and
// validated, then imported with the fingerprint it was read with, so a
// change in between fails the import, and the script reads it again.
//
// It needs gcloud, signed in as an account that may update the URL map
// (scripts/flake-latest/provision.sh). `--dry-run` prints the new map and
// changes nothing.

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const PROJECT = "dassie-390023";
export const URL_MAP = "get-dassie-land";
export const HOST = "get.dassie.land";
/** The path matcher this script owns. */
export const MATCHER = "get-dassie-land-flake";
export const PATH = "/flake/latest.tar.gz";
/** The backend bucket over the mirror's bucket without Cloud CDN. */
export const UNCACHED =
  `https://www.googleapis.com/compute/v1/projects/${PROJECT}/global/backendBuckets/get-dassie-land-uncached`;

/** A URL map as `gcloud compute url-maps describe --format=json` gives it:
 * only the parts read here are typed. */
export interface UrlMap {
  defaultService?: string;
  hostRules?: { hosts: string[]; pathMatcher: string }[];
  pathMatchers?: {
    name: string;
    defaultService?: string;
    [key: string]: unknown;
  }[];
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
  // first time.
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
        service: UNCACHED,
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
  return {
    ...rest,
    hostRules: existing
      ? hostRules
      : [...hostRules, { hosts: [HOST], pathMatcher: MATCHER }],
    pathMatchers: existing
      ? pathMatchers.map((m) => m.name === MATCHER ? matcher : m)
      : [...pathMatchers, matcher],
  };
}

/** Whether `map`'s route already names `version`. */
export function routes(map: UrlMap, version: string): boolean {
  const matcher = map.pathMatchers?.find((m) => m.name === MATCHER);
  const routed = map.hostRules?.some((rule) =>
    rule.pathMatcher === MATCHER && rule.hosts.includes(HOST)
  );
  if (!matcher || !routed) return false;
  return JSON.stringify(matcher) ===
    JSON.stringify(
      routeFlake(map, version).pathMatchers!.find((m) => m.name === MATCHER),
    );
}

function gcloud(args: string[]): Promise<string> {
  const child = spawn("gcloud", [...args, `--project=${PROJECT}`], {
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
        new Error(`gcloud ${args.slice(0, 3).join(" ")} failed (${code})`),
      ));
  });
}

/** Reads the map, routes the flake and imports the map, once more if the
 * map changed in between. */
async function apply(version: string, dryRun: boolean): Promise<void> {
  const work = await mkdtemp(join(tmpdir(), "dassie-flake-latest-"));
  try {
    for (let attempt = 1;; attempt++) {
      const map = JSON.parse(
        await gcloud([
          "compute",
          "url-maps",
          "describe",
          URL_MAP,
          "--format=json",
        ]),
      ) as UrlMap;
      if (routes(map, version)) {
        console.log(`${PATH} already names ${version}`);
        return;
      }
      const next = JSON.stringify(routeFlake(map, version), null, 2);
      if (dryRun) {
        console.log(next);
        return;
      }
      const source = join(work, "url-map.json");
      await writeFile(source, next);
      await gcloud([
        "compute",
        "url-maps",
        "validate",
        `--source=${source}`,
        "--global",
      ]);
      try {
        await gcloud([
          "compute",
          "url-maps",
          "import",
          URL_MAP,
          `--source=${source}`,
          "--global",
          "--quiet",
        ]);
        console.log(`${PATH} names ${version}`);
        return;
      } catch (error) {
        if (attempt >= 2) throw error;
        console.error("the import failed; reading the map again");
      }
    }
  } finally {
    await rm(work, { recursive: true, force: true });
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
