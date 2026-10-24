import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { LedgerConnection } from "@dassie/rpc/proto";
import { connectionText } from "./connection.ts";

test("a ledger's connection is told only when it is not in step", () => {
  const text = (connection: LedgerConnection, connectionProblem = "") =>
    connectionText({ connection, connectionProblem, faucet: true });
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

// What a ledger that connects or catches up waits for, it says: such as an
// XRPL account without a validator list, or on a server without proofs.
test("a ledger says what it waits for", () => {
  const text = (connection: LedgerConnection, connectionProblem: string) =>
    connectionText({ connection, connectionProblem, faucet: false });
  assertEquals(
    text(LedgerConnection.SYNCING, "the server serves no proofs"),
    "Catching up with the ledger: waiting, as the server serves no proofs.",
  );
  assertEquals(
    text(LedgerConnection.CONNECTING, "the server did not answer"),
    "Connecting to the ledger: waiting, as the server did not answer.",
  );
  assertEquals(
    text(LedgerConnection.SYNCING, ""),
    "Catching up with the ledger…",
  );
});

// Mainnet has no faucet: an account there waits for the owner's XRP alone.
test("an account without a faucet is told to wait for XRP alone", () => {
  const unfunded = (faucet: boolean) =>
    connectionText({
      connection: LedgerConnection.UNFUNDED,
      connectionProblem: "",
      faucet,
    });
  assertEquals(unfunded(false), "The account does not exist yet: send it XRP.");
  assertEquals(unfunded(true)?.includes("testnet faucet"), true);
});
