import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  LedgerRemovalBlocker_Kind as Kind,
  LedgerRemovalBlockerSchema,
} from "@dassie/rpc/proto";
import { expectedConfirmation, removalChecklist } from "./removal.ts";

const blocker = (kind: Kind, count: number) =>
  create(LedgerRemovalBlockerSchema, { kind, count });

test("the confirmation names the scheme", () => {
  assertEquals(expectedConfirmation("stub"), "DELETE STUB");
  assertEquals(expectedConfirmation("xrpl-testnet"), "DELETE XRPL-TESTNET");
});

test("the checklist shows what blocks the removal", () => {
  assertEquals(removalChecklist([]).map((item) => item.state), ["valid"]);
  const items = removalChecklist([
    blocker(Kind.PEERS, 2),
    blocker(Kind.BTP_TOKENS, 1),
    blocker(Kind.FINAL_SETTLEMENTS, 3),
  ]);
  assertEquals(items.map((item) => [item.id, item.state]), [
    ["peers", "invalid"],
    ["btp-tokens", "invalid"],
    ["final-settlements", "invalid"],
  ]);
  assertEquals(
    items[0].label,
    "Settlement scheme is still in use for 2 peers",
  );
  assertEquals(
    items[1].label,
    "1 BTP token uses its ledger; revoke it first",
  );
});
