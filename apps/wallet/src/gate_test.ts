import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { GetBasicStateResponseSchema, NodeState } from "@dassie/rpc/proto";
import { decideScreen, isSetupPath, ownerScreen } from "./gate.ts";

const basic = (state: NodeState) =>
  create(GetBasicStateResponseSchema, { state });
const token = "ab".repeat(32);

test("the node's state decides the screen", () => {
  assertEquals(decideScreen(undefined, undefined, "/"), { kind: "loading" });
  assertEquals(decideScreen(undefined, "refused", "/"), {
    kind: "unreachable",
    message: "refused",
  });
  assertEquals(
    decideScreen(
      basic(NodeState.UNINITIALIZED),
      undefined,
      `/setup/${token.toUpperCase()}`,
    ),
    { kind: "setup", token },
  );
  assertEquals(
    decideScreen(basic(NodeState.UNINITIALIZED), undefined, "/setup/short"),
    { kind: "setup-info" },
  );
  assertEquals(
    decideScreen(basic(NodeState.ANONYMOUS), undefined, `/setup/${token}`),
    { kind: "login" },
  );
  assertEquals(
    decideScreen(basic(NodeState.AUTHENTICATED), undefined, "/ledgers"),
    { kind: "owner" },
  );
  assertEquals(decideScreen(basic(99 as NodeState), undefined, "/"), {
    kind: "unknown",
  });
});

// UI-7: the first ledger shows the wallet at once, without polling.
test("the owner chooses a first ledger, then sees the wallet", () => {
  assertEquals(ownerScreen(undefined), "loading");
  assertEquals(ownerScreen([]), "onboarding");
  assertEquals(ownerScreen([{ id: "stub+usd" }]), "wallet");
  assertEquals(isSetupPath(`/setup/${token}`), true);
  assertEquals(isSetupPath("/settings"), false);
});
