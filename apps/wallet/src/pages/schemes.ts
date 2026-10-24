/** Settlement schemes as the wallet names and describes them. Disabled
 * test RLUSD (`xrpl-testnet-rlusd`) is not among them: no node runs or
 * offers it (spec/decisions/issued-assets.md). */

const NAMES: Record<string, string> = {
  stub: "Stub",
  xrpl: "XRP Ledger",
  "xrpl-rlusd": "RLUSD on the XRP Ledger",
  "xrpl-testnet": "XRP Ledger Testnet",
  "xrpl-testnet-dsusd": "DSUSD on the XRP Ledger Testnet",
};

const DESCRIPTIONS: Record<string, string> = {
  stub:
    "Stub Settlement simulates settlement delays but does not actually connect to any external ledger.",
  xrpl:
    "Settles in XRP on the XRP Ledger mainnet: real money. There is no faucet: send XRP to the account to fund it; the ledger holds back a reserve.",
  "xrpl-rlusd":
    "Settles in RLUSD, Ripple's dollar token, on the XRP Ledger mainnet: real money, from the same account as XRP. The account pays its fees and its reserve in XRP, which you send to it: there is no faucet. The node pauses it while RLUSD's XRP/RLUSD AMM pool prices it off a dollar.",
  "xrpl-testnet":
    "Settles in test XRP on the XRP Ledger testnet. The testnet faucet funds the account; the ledger holds back a reserve.",
  "xrpl-testnet-dsusd":
    "Settles in test DSUSD, Dassie's testnet dollar token, on the XRP Ledger testnet, from the same account as XRP. The account pays its fees and its reserve in XRP, from the testnet faucet; DSUSD is bought with XRP on the testnet's XRP/DSUSD AMM pool, as there is no faucet for it. The node pauses it while that pool prices it off a dollar.",
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
  /** The AMM pool whose price the node's peg guard reads, such as
   * "XRP/RLUSD", for a token that has one. */
  readonly pool?: string;
}

const TOKENS: Record<string, TokenScheme> = {
  "xrpl-rlusd": {
    token: "RLUSD",
    issuerName: "Ripple",
    gas: "XRP",
    pool: "XRP/RLUSD",
  },
  "xrpl-testnet-dsusd": {
    token: "DSUSD",
    issuerName: "Dassie",
    gas: "XRP",
    pool: "XRP/DSUSD",
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
