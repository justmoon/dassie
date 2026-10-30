/** Assets and settlement schemes as the wallet names and describes them.
 * Disabled test RLUSD (`xrpl-testnet-rlusd`) is not among them: no node
 * runs or offers it (spec/decisions/issued-assets.md). */

import { Realm } from "@dassie/rpc/proto";

const NAMES: Record<string, string> = {
  stub: "Stub",
  "xrpl-xrp": "XRP Ledger",
  "xrpl-rlusd": "RLUSD on the XRP Ledger",
  "xrpl-testnet-xrp": "XRP Ledger Testnet",
  "xrpl-testnet-dsusd": "DSUSD on the XRP Ledger Testnet",
  "hub-xrpl-testnet-xrp": "XRP in a settlement hub on the XRP Ledger Testnet",
  "hub-xrpl-testnet-dsusd":
    "DSUSD in a settlement hub on the XRP Ledger Testnet",
  "hub-xrpl-xrp": "XRP in a settlement hub on the XRP Ledger",
  "hub-xrpl-rlusd": "RLUSD in a settlement hub on the XRP Ledger",
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

/** What a scheme that settles in a token settles in and pays gas with;
 * nothing for one that settles in its ledger's own asset. */
export function tokenScheme(id: string): TokenScheme | undefined {
  return TOKENS[id];
}

/** An asset the owner may hold, for people: its name, and what holding it
 * means, in a sentence or three. */
export interface AssetText {
  readonly label: string;
  readonly description: string;
}

/** The assets the node offers (`ListAssets`), by name and realm: the
 * owner chooses assets, never schemes
 * (spec/decisions/owner-chooses-assets.md). */
const ASSETS: Record<string, Partial<Record<Realm, AssetText>>> = {
  usd: {
    [Realm.TEST]: {
      label: "Stub dollars",
      description:
        "Test dollars without an underlying ledger, for development and tests: peers trust each other without limit and never settle, so nothing real is held. Only development builds offer them.",
    },
  },
  xrp: {
    [Realm.TEST]: {
      label: "Test XRP",
      description:
        "XRP on the XRP Ledger testnet, free from the testnet's faucet. Your node joins the testnet's XRP hub to pay and be paid in it.",
    },
    [Realm.LIVE]: {
      label: "XRP",
      description:
        "XRP on the XRP Ledger: real money, which you send to your node's account there. Your node joins an XRP hub to pay and be paid in it.",
    },
  },
  dsusd: {
    [Realm.TEST]: {
      label: "Test DSUSD",
      description:
        "Dassie's test dollar on the XRP Ledger testnet, in the same account as your XRP. It has no faucet: you buy it with test XRP on the testnet's pool. Your node joins the testnet's DSUSD hub.",
    },
  },
  rlusd: {
    [Realm.LIVE]: {
      label: "RLUSD",
      description:
        "Ripple's dollar token on the XRP Ledger: real money, in the same account as your XRP, which you send to it. Your node joins an RLUSD hub to pay and be paid in it.",
    },
  },
};

/** An asset's name and description for people, by its name and realm. */
export function assetText(name: string, realm: Realm): AssetText | undefined {
  return ASSETS[name]?.[realm] ?? ASSETS[name]?.[Realm.TEST];
}
