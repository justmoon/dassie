import { assertEquals } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";
import { connectionText } from "./connection.ts";

Deno.test("a ledger's connection is told only when it is not in step", () => {
  const text = (connection: LedgerConnection, connectionProblem = "") =>
    connectionText({ connection, connectionProblem });
  assertEquals(text(LedgerConnection.SYNCED), undefined);
  assertEquals(text(LedgerConnection.UNSPECIFIED), undefined);
  assertEquals(text(LedgerConnection.CONNECTING), "Connecting to the ledger…");
  assertEquals(
    text(LedgerConnection.FAILED, "the books disagree"),
    "Something is wrong with the ledger: the books disagree.",
  );
  assertEquals(
    text(LedgerConnection.UNFUNDED)?.startsWith("The account"),
    true,
  );
});
