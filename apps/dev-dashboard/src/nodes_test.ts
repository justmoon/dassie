import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { NodeSchema } from "@dassie/rpc/dev";
import { nodePath, walletLink } from "./nodes.ts";

test("switching nodes keeps the debug tab", () => {
  assertEquals(
    nodePath("d2", "/nodes/d1/debug/ledger"),
    "/nodes/d2/debug/ledger",
  );
  assertEquals(
    nodePath("d2", "/nodes/d1/debug/database/node_config"),
    "/nodes/d2/debug/database",
  );
  assertEquals(nodePath("d2", "/logs"), "/nodes/d2");
  assertEquals(nodePath("d2", "/nodes/host"), "/nodes/d2");
});

test("a wallet link carries the login proof in its fragment", () => {
  const node = create(NodeSchema, {
    url: "https://d1.localhost",
    loginProofHex: "ab".repeat(32),
  });
  assertEquals(
    walletLink(node),
    `https://d1.localhost/#dassie-dev-login=${"ab".repeat(32)}`,
  );
});
