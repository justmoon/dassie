// scripts/flake-latest.ts's change of the mirror's URL map, on a map shaped
// as the mirror's is: get.dassie.land on the map's default service, with no
// host rule, sh.dassie.land rewritten to install.sh, and the website's
// entries (scripts/website/provision.sh).

import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import {
  HOST,
  MATCHER,
  PATH,
  routeFlake,
  routes,
  type UrlMap,
} from "./flake-latest.ts";

const MIRROR =
  "https://www.googleapis.com/compute/v1/projects/dassie-390023/global/backendBuckets/get-dassie-land";
const WEBSITE =
  "https://www.googleapis.com/compute/v1/projects/dassie-390023/global/backendBuckets/dassie-website";

function mirrorMap(): UrlMap {
  return {
    id: "1234",
    kind: "compute#urlMap",
    selfLink:
      "https://www.googleapis.com/compute/v1/projects/dassie-390023/global/urlMaps/get-dassie-land",
    creationTimestamp: "2023-06-01T00:00:00.000-07:00",
    name: "get-dassie-land",
    fingerprint: "Qbb-HCFnKFk=",
    defaultService: MIRROR,
    hostRules: [
      { hosts: ["sh.dassie.land"], pathMatcher: "path-matcher-1" },
      { hosts: ["dassie.land"], pathMatcher: "dassie-land" },
      { hosts: ["www.dassie.land"], pathMatcher: "www-dassie-land" },
    ],
    pathMatchers: [
      {
        name: "path-matcher-1",
        defaultService: MIRROR,
        routeRules: [{
          priority: 1,
          matchRules: [{ fullPathMatch: "/" }],
          service: MIRROR,
          routeAction: { urlRewrite: { pathPrefixRewrite: "/install.sh" } },
        }],
      },
      {
        name: "dassie-land",
        defaultService: WEBSITE,
        routeRules: [{
          priority: 1,
          matchRules: [{ fullPathMatch: "/docs/start/quick-start" }],
          urlRedirect: { pathRedirect: "/docs/start/quick-start/" },
        }, {
          priority: 2,
          matchRules: [
            { fullPathMatch: "/guides/quick-start" },
            { fullPathMatch: "/guides/quick-start/" },
            { fullPathMatch: "/guides/quick-start/index.html" },
          ],
          urlRedirect: { pathRedirect: "/docs/start/quick-start/" },
        }],
      },
      {
        name: "www-dassie-land",
        defaultUrlRedirect: { hostRedirect: "dassie.land" },
      },
    ],
  };
}

test("routes the flake on get.dassie.land, keeping its service", () => {
  const map = routeFlake(mirrorMap(), "0.2.2");
  const ours = map.pathMatchers!.find((m) => m.name === MATCHER)!;
  assertEquals(ours.defaultService, MIRROR);
  assertEquals(ours.routeRules, [{
    priority: 1,
    matchRules: [{ fullPathMatch: PATH }],
    service: MIRROR,
    routeAction: { urlRewrite: { pathPrefixRewrite: "/0.2.2/flake.tar.gz" } },
    headerAction: {
      responseHeadersToAdd: [
        {
          headerName: "Link",
          headerValue:
            '<https://get.dassie.land/0.2.2/flake.tar.gz>; rel="immutable"',
          replace: true,
        },
        {
          headerName: "Cache-Control",
          headerValue: "no-cache, max-age=0",
          replace: true,
        },
      ],
    },
  }]);
  assertEquals(map.hostRules!.at(-1), { hosts: [HOST], pathMatcher: MATCHER });
});

test("leaves every other entry as it is, and keeps the fingerprint", () => {
  const before = mirrorMap();
  const map = routeFlake(before, "0.2.2");
  assertEquals(map.hostRules!.slice(0, 3), before.hostRules);
  assertEquals(map.pathMatchers!.slice(0, 3), before.pathMatchers);
  assertEquals(map.defaultService, MIRROR);
  assertEquals(map.fingerprint, "Qbb-HCFnKFk=");
  assertEquals(map.name, "get-dassie-land");
  for (const key of ["id", "kind", "selfLink", "creationTimestamp"]) {
    assertEquals(key in map, false, key);
  }
});

test("moves the route to a later release, in place", () => {
  const first = routeFlake(mirrorMap(), "0.2.2");
  const second = routeFlake(first, "0.2.3");
  assertEquals(second.hostRules, first.hostRules);
  assertEquals(second.pathMatchers!.length, first.pathMatchers!.length);
  const ours = second.pathMatchers!.find((m) => m.name === MATCHER)!;
  assertEquals(ours.defaultService, MIRROR);
  assertEquals(routes(second, "0.2.3"), true);
  assertEquals(routes(second, "0.2.2"), false);
});

test("is a no-op for the version it names", () => {
  const map = routeFlake(mirrorMap(), "0.2.2");
  assertEquals(routes(map, "0.2.2"), true);
  assertEquals(routes(mirrorMap(), "0.2.2"), false);
  assertEquals(routeFlake(map, "0.2.2"), map);
});

test("reads a route as GCP gives it back: other key order, more fields", () => {
  const map = routeFlake(mirrorMap(), "0.2.2");
  const ours = map.pathMatchers!.find((m) => m.name === MATCHER)!;
  const [rule] = ours.routeRules!;
  ours.routeRules = [{
    headerAction: {
      responseHeadersToAdd: rule.headerAction!.responseHeadersToAdd!
        .toReversed(),
    },
    routeAction: rule.routeAction,
    service: rule.service,
    matchRules: [{ fullPathMatch: PATH, ignoreCase: false }],
    priority: 1,
  }];
  assertEquals(routes(map, "0.2.2"), true);
});

test("puts the host rule back if it went", () => {
  const map = routeFlake(mirrorMap(), "0.2.2");
  map.hostRules = map.hostRules!.filter((rule) => rule.pathMatcher !== MATCHER);
  assertEquals(routes(map, "0.2.2"), false);
  const again = routeFlake(map, "0.2.2");
  assertEquals(again.hostRules!.at(-1), {
    hosts: [HOST],
    pathMatcher: MATCHER,
  });
  assertEquals(routes(again, "0.2.2"), true);
});

test("refuses what is not a release", () => {
  for (
    const version of [
      "canary-2026-10-15-041530-15bd7aa",
      "0.2",
      "../0.2.2",
      "",
    ]
  ) {
    assertThrows(
      () => routeFlake(mirrorMap(), version),
      Error,
      "not a release",
    );
  }
});

test("refuses a map where something else serves get.dassie.land", () => {
  for (const hosts of [[HOST], ["*.dassie.land"], ["*"]]) {
    const map = mirrorMap();
    map.hostRules!.push({ hosts, pathMatcher: "dassie-land" });
    assertThrows(
      () => routeFlake(map, "0.2.2"),
      Error,
      "route /flake/latest.tar.gz by hand",
    );
  }
});

test("refuses a default that is more than a service", () => {
  for (
    const extra of [
      { defaultRouteAction: { urlRewrite: { hostRewrite: "x" } } },
      { defaultUrlRedirect: { httpsRedirect: true } },
    ]
  ) {
    assertThrows(
      () => routeFlake({ ...mirrorMap(), ...extra }, "0.2.2"),
      Error,
      "more than a service",
    );
  }
});
