/** Settlement schemes as the wallet names and describes them. */

const NAMES: Record<string, string> = {
  stub: "Stub",
  xrpl: "XRP Ledger",
  "xrpl-testnet": "XRP Ledger Testnet",
};

const DESCRIPTIONS: Record<string, string> = {
  stub:
    "Stub Settlement simulates settlement delays but does not actually connect to any external ledger.",
  "xrpl-testnet":
    "Settles in test XRP on the XRP Ledger testnet. The testnet faucet funds the account; the ledger holds back a reserve.",
};

/** A scheme's name for people. */
export function schemeName(id: string, fallback?: string): string {
  return NAMES[id] ?? fallback ?? id;
}

/** What a scheme does, if the wallet knows. */
export function schemeDescription(id: string): string | undefined {
  return DESCRIPTIONS[id];
}
