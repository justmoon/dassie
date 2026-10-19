/**
 * How a ledger's scheme stands with its underlying ledger, for people
 * (spec/areas/wallet.md#pages): connecting, catching up, waiting for funds,
 * or failed.
 */

import { LedgerConnection } from "@dassie/rpc/proto";

/** What to say about a ledger's connection; nothing while it is synced. */
export function connectionText(ledger: {
  connection: LedgerConnection;
  connectionProblem: string;
}): string | undefined {
  switch (ledger.connection) {
    case LedgerConnection.CONNECTING:
      return "Connecting to the ledger…";
    case LedgerConnection.SYNCING:
      return "Catching up with the ledger…";
    case LedgerConnection.UNFUNDED:
      return "The account does not exist yet: send it XRP, or wait for the testnet faucet.";
    case LedgerConnection.FAILED:
      return `Something is wrong with the ledger: ${
        ledger.connectionProblem || "unknown"
      }.`;
    default:
      return undefined;
  }
}
