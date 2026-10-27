import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  LedgerRemovalBlocker_Kind as Kind,
  LedgerRemovalBlockerSchema,
} from "@dassie/rpc/proto";
import {
  expectedConfirmation,
  removalChecklist,
  removingNotice,
} from "./removal.ts";

const blocker = (kind: Kind, count: number) =>
  create(LedgerRemovalBlockerSchema, { kind, count });

test("the confirmation names the scheme", () => {
  assertEquals(expectedConfirmation("stub"), "DELETE STUB");
  assertEquals(
    expectedConfirmation("xrpl-testnet-xrp"),
    "DELETE XRPL-TESTNET-XRP",
  );
});

test("the checklist shows what blocks the removal", () => {
  assertEquals(removalChecklist([]).map((item) => item.state), ["valid"]);
  const items = removalChecklist([
    blocker(Kind.BTP_TOKENS, 1),
    blocker(Kind.FINAL_SETTLEMENTS, 3),
  ]);
  assertEquals(items.map((item) => [item.id, item.state]), [
    ["peers", "valid"],
    ["btp-tokens", "invalid"],
    ["final-settlements", "invalid"],
  ]);
  assertEquals(
    items[0].label,
    "Peers that settle with it are asked to settle once, then your node stops settling with them on it",
  );
  assertEquals(
    items[1].label,
    "1 BTP token uses its ledger; revoke it first",
  );
});

test("a move of the ledger's hub on its way blocks the removal", () => {
  const items = removalChecklist([blocker(Kind.HUB_MOVE, 1)]);
  assertEquals(items.map((item) => [item.id, item.state]), [
    ["peers", "valid"],
    ["hub-move", "invalid"],
  ]);
  assertEquals(
    items[1].label,
    "Funds are on their way between its settlement hub and the ledger that funds it; wait for them, or give the move up on the Hubs page once it is late",
  );
});

test("a ledger being removed settles up with its peers first", () => {
  assertEquals(removingNotice(undefined), undefined);
  assertEquals(
    removingNotice(2)?.text,
    "Your node is settling up with 2 peers on it first, which takes a few minutes at most. Then it goes.",
  );
  assertEquals(
    removingNotice(0)?.text,
    "Your node removes it as soon as nothing else uses it.",
  );
});
