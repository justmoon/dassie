import { assertEquals } from "@std/assert";
import {
  banText,
  costParts,
  costText,
  parseShare,
  RECENT_BAN_MS,
  recentAutomaticBans,
  shareText,
} from "./grace.ts";

const cost = {
  assetCode: "USD",
  assetScale: 9,
  refused: 600_000_000n,
  overdue: 20_000_000n,
  grace: 550_000_000n,
};

Deno.test("a cost reads as a share of its limit, with its parts", () => {
  assertEquals(costText(cost), "$ 0.62 of $ 0.55");
  assertEquals(costParts(cost), "$ 0.60 refused, $ 0.02 overdue");
  assertEquals(costParts({ ...cost, refused: 0n, overdue: 0n }), "");
});

Deno.test("a ban says why", () => {
  assertEquals(
    banText({ reason: "grace-limit", bannedAtUnixMs: 1n, cost }),
    "Banned automatically: it cost $ 0.62 in 30 days, over its grace limit of $ 0.55.",
  );
  assertEquals(
    banText({ reason: "owner", bannedAtUnixMs: 1n }),
    "Banned by you.",
  );
  assertEquals(banText({ reason: "", bannedAtUnixMs: 0n }), "Banned.");
});

Deno.test("the dashboard tells of automatic bans of the last week", () => {
  const now = 10 * RECENT_BAN_MS;
  const node = (id: string, reason: string, at: number) => ({
    id,
    ban: { reason, bannedAtUnixMs: BigInt(at) },
  });
  const recent = recentAutomaticBans([
    node("old", "grace-limit", now - RECENT_BAN_MS - 1),
    node("owner", "owner", now - 1),
    node("first", "grace-limit", now - 100),
    node("later", "grace-limit", now - 10),
    { id: "none" },
  ], now);
  assertEquals(recent.map((n) => n.id), ["later", "first"]);
});

Deno.test("shares are percentages of the credit", () => {
  assertEquals(shareText(1_000_000), "100%");
  assertEquals(shareText(12_345), "1.23%");
  assertEquals(parseShare("100"), 1_000_000);
  assertEquals(parseShare(" 50 % "), 500_000);
  assertEquals(parseShare("0.0001"), 1);
  assertEquals(parseShare("1000"), 10_000_000);
  for (const bad of ["0", "1000.01", "-5", "abc", "", "1.23456"]) {
    assertEquals(parseShare(bad), undefined, bad);
  }
});
