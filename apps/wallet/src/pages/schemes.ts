/** Settlement schemes as the wallet names and describes them. */

const NAMES: Record<string, string> = {
  stub: "Stub",
  xrpl: "XRP Ledger",
  "xrpl-testnet": "XRP Ledger Testnet",
  "xrpl-testnet-rlusd": "RLUSD on the XRP Ledger Testnet",
  "xrpl-testnet-dsusd": "DSUSD on the XRP Ledger Testnet",
};

const DESCRIPTIONS: Record<string, string> = {
  stub:
    "Stub Settlement simulates settlement delays but does not actually connect to any external ledger.",
  "xrpl-testnet":
    "Settles in test XRP on the XRP Ledger testnet. The testnet faucet funds the account; the ledger holds back a reserve.",
  "xrpl-testnet-rlusd":
    "Settles in test RLUSD, Ripple's dollar token, on the XRP Ledger testnet, from the same account as XRP. The account pays its fees and its reserve in XRP, from the testnet faucet; RLUSD is sent to it.",
  "xrpl-testnet-dsusd":
    "Settles in test DSUSD, Dassie's testnet dollar token, on the XRP Ledger testnet, from the same account as XRP. The account pays its fees and its reserve in XRP, from the testnet faucet; DSUSD is bought with XRP on the testnet's XRP/DSUSD AMM pool, as there is no faucet for it.",
};

/**
 * A scheme that settles in a token on a ledger whose own asset pays its
 * fees and reserve, as gas (spec/decisions/external-assets.md).
 */
export interface TokenScheme {
  /** The token it settles in, such as "RLUSD". */
  readonly token: string;
  /** Who issues it, for people. */
  readonly issuerName: string;
  /** The ledger's own asset, which pays the fees: "XRP". */
  readonly gas: string;
  /** How the owner gets the token, when it is not simply sent: for
   * people, a sentence or two. */
  readonly obtain?: string;
}

const TOKENS: Record<string, TokenScheme> = {
  "xrpl-testnet-rlusd": { token: "RLUSD", issuerName: "Ripple", gas: "XRP" },
  "xrpl-testnet-dsusd": {
    token: "DSUSD",
    issuerName: "Dassie",
    gas: "XRP",
    obtain:
      "DSUSD has no faucet: buy it with XRP on the testnet's XRP/DSUSD AMM pool, from any XRP Ledger wallet. A payment to this address whose amount is DSUSD, paid for with XRP, swaps through the pool and arrives here as DSUSD.",
  },
};

/** A scheme's name for people. */
export function schemeName(id: string, fallback?: string): string {
  return NAMES[id] ?? fallback ?? id;
}

/** What a scheme does, if the wallet knows. */
export function schemeDescription(id: string): string | undefined {
  return DESCRIPTIONS[id];
}

/** What a scheme that settles in a token settles in and pays gas with;
 * nothing for one that settles in its ledger's own asset. */
export function tokenScheme(id: string): TokenScheme | undefined {
  return TOKENS[id];
}
