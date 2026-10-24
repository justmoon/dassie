/**
 * How a ledger's scheme stands with its underlying ledger, for people
 * (spec/areas/wallet.md#pages): connecting, catching up, and what it waits
 * for if something keeps it, waiting for funds, or failed.
 */

import { LedgerConnection } from "@dassie/rpc/proto";

/** What to say about a ledger's connection; nothing while it is synced. */
export function connectionText(ledger: {
  connection: LedgerConnection;
  connectionProblem: string;
  /** Whether a faucet funds it: the testnet's; mainnet has none. */
  faucet: boolean;
}): string | undefined {
  switch (ledger.connection) {
    case LedgerConnection.CONNECTING:
      return waiting("Connecting to the ledger", ledger.connectionProblem);
    case LedgerConnection.SYNCING:
      return waiting("Catching up with the ledger", ledger.connectionProblem);
    case LedgerConnection.UNFUNDED:
      return ledger.faucet
        ? "The account does not exist yet: send it XRP, or wait for the testnet faucet."
        : "The account does not exist yet: send it XRP.";
    case LedgerConnection.FAILED:
      return `Something is wrong with the ledger: ${
        ledger.connectionProblem || "unknown"
      }.`;
    default:
      return undefined;
  }
}

/** `doing`, and what keeps it, if anything: such as a validator list the
 * node has none of yet. */
function waiting(doing: string, why: string): string {
  return why ? `${doing}: waiting, as ${why}.` : `${doing}…`;
}
