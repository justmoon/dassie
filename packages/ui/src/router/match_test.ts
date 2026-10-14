import { assertEquals } from "@std/assert";
import { join, match, matchRoutes, within } from "./match.ts";

Deno.test("literal and parameter segments", () => {
  assertEquals(match("/ledgers", "/ledgers"), {});
  assertEquals(match("/ledgers", "/ledgers/"), {});
  assertEquals(match("/ledgers", "/ledger"), undefined);
  assertEquals(match("/ledgers/delete/:id", "/ledgers/delete/stub"), {
    id: "stub",
  });
  assertEquals(match("/ledgers/delete/:id", "/ledgers/delete"), undefined);
  assertEquals(match("/ledgers/delete/:id", "/ledgers/delete/a/b"), undefined);
  assertEquals(match("/", "/"), {});
});

Deno.test("parameters are decoded, and bad encodings do not match", () => {
  assertEquals(match("/x/:id", "/x/stub%2Busd"), { id: "stub+usd" });
  assertEquals(match("/x/:id", "/x/%E0%A4%A"), undefined);
});

Deno.test("a rest parameter takes the rest of the path", () => {
  const pattern = "/debug/ledger/:ledger/account/*path";
  assertEquals(
    match(pattern, "/debug/ledger/stub+usd/account/equity/owner/btp/3"),
    { ledger: "stub+usd", path: "equity/owner/btp/3" },
  );
  assertEquals(match("/a/*rest", "/a"), { rest: "" });
});

Deno.test("routes match in order", () => {
  const routes = [
    { path: "/network/register", component: "register" },
    { path: "/network/:node", component: "node" },
    { path: "/network", component: "network" },
  ];
  assertEquals(
    matchRoutes(routes, "/network/register")?.route.component,
    "register",
  );
  assertEquals(matchRoutes(routes, "/network/d1")?.params, { node: "d1" });
  assertEquals(matchRoutes(routes, "/nowhere"), undefined);
});

Deno.test("bases", () => {
  assertEquals(join("", "/debug/logs"), "/debug/logs");
  assertEquals(join("/nodes/d1", "/debug/logs"), "/nodes/d1/debug/logs");
  assertEquals(join("/nodes/d1/", "/"), "/nodes/d1");
  assertEquals(within("", "/x"), "/x");
  assertEquals(within("/nodes/d1", "/nodes/d1/debug"), "/debug");
  assertEquals(within("/nodes/d1", "/nodes/d1"), "/");
  assertEquals(within("/nodes/d1", "/nodes/d10/debug"), undefined);
});
