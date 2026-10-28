/**
 * The settlement hubs the node is in, as the Hubs page shows them
 * (spec/areas/wallet.md#pages, spec/areas/hubs.md#in-the-node): each hub's
 * account, holds, floor and ceiling, the move on its way, and its payout
 * once it ended or stalled, as the node reports them. The wallet holds no
 * rule of the node's: when to move funds, what a move may be and when one
 * is late are the node's.
 */

import {
  ErrorCode,
  type GetHubPayoutResponse,
  type HubMove,
  HubMove_Direction as Direction,
  type HubStatus,
  LedgerConnection,
} from "@dassie/rpc/proto";
import { type Currency, currencyOf, formatAmount } from "@dassie/ui/currency";
import { relativeTime } from "@dassie/ui/format";

/** How often the page asks the node for its hubs while it shows them: no
 * stream carries them (spec/areas/wallet.md#live-data). */
export const HUB_POLL_MS = 5_000;

/** How often it asks for each hub's payout: the node asks the validators
 * for theirs only every minute (spec/areas/hubs.md#payout-bundles). */
export const PAYOUT_POLL_MS = 60_000;

/**
 * The currency of a hub's amounts: the asset of its scheme's ledger, as the
 * node reports the ledger, never assumed (spec/areas/wallet.md#amounts-and-currencies).
 * Nothing while the node lists no ledger of the scheme.
 */
export function hubCurrency(
  scheme: string,
  ledgers: readonly {
    settlementScheme: string;
    assetCode: string;
    assetScale: number;
  }[],
): Currency | undefined {
  const ledger = ledgers.find((ledger) => ledger.settlementScheme === scheme);
  return ledger && currencyOf(ledger.assetCode, ledger.assetScale);
}

/** How the node's account stands with the hub, for people; nothing while
 * it is up to date. */
export function hubConnectionText(
  connection: LedgerConnection,
): string | undefined {
  switch (connection) {
    case LedgerConnection.UNFUNDED:
      return "You have no account in this hub yet. Your first deposit opens it.";
    case LedgerConnection.CONNECTING:
      return "Reaching the hub's validators…";
    case LedgerConnection.SYNCING:
      return "Reading what the hub agreed…";
    case LedgerConnection.FAILED:
      return "Your node cannot follow this hub right now.";
    default:
      return undefined;
  }
}

/**
 * What the issuer of a token hub's asset holds of the hub's escrow, for
 * people (spec/areas/hubs.md#a-frozen-hub): a freeze, and since when, or a
 * hold that keeps an ended hub's payout back. Nothing while it holds
 * nothing, as for every XRP hub.
 */
export function hubIssuerText(
  status: Pick<HubStatus, "frozenSinceUnixMs" | "escrowHeld">,
  nowMs: number,
): string | undefined {
  if (status.frozenSinceUnixMs !== undefined) {
    return `The issuer of this hub's token holds the hub's escrow, which froze the hub ${
      relativeTime(status.frozenSinceUnixMs, nowMs)
    }. Until it lets go, no payment crosses this hub and nothing moves into or out of it. Your node still claims what peers owe it for payments it passed on before the freeze, and the rest waits for the thaw.`;
  }
  if (status.escrowHeld) {
    return "The issuer of this hub's token holds the hub's escrow. The escrow pays out nothing until the issuer lets go.";
  }
  return undefined;
}

/** How the node's account stands with the hub, for people, with the
 * issuer's freeze first: a frozen hub's account follows the hub all the
 * same, but takes on nothing until the thaw, which the freeze's own text
 * says. One the node cannot follow says so all the same. Nothing while it
 * is up to date. */
export function hubStandingText(
  status: Pick<HubStatus, "connection" | "frozenSinceUnixMs">,
): string | undefined {
  if (
    status.frozenSinceUnixMs !== undefined &&
    status.connection !== LedgerConnection.FAILED
  ) {
    return undefined;
  }
  return hubConnectionText(status.connection);
}

/** How many validators answered, for people. */
export function validatorsText(validators: number, answering: number): string {
  return `${answering} of ${validators} ${
    validators === 1 ? "validator" : "validators"
  } answering`;
}

/** An amount as text, in its currency. */
function amount(value: bigint, currency: Currency): string {
  return formatAmount(value, currency).text;
}

/**
 * The move on its way, for people: which way, how much, who asked, and
 * since when. Late, it says the node waits and pays nothing again.
 */
export function moveText(
  move: HubMove,
  currency: Currency,
  nowMs: number,
): string {
  const deposit = move.direction === Direction.DEPOSIT;
  const what = `${deposit ? "Depositing" : "Withdrawing"} ${
    amount(move.amount, currency)
  } ${deposit ? "into" : "out of"} the hub`;
  const who = move.byOwner ? "you asked" : "your node moves it by itself";
  const since = move.appliedAtUnixMs !== undefined
    ? `the XRP Ledger applied it ${relativeTime(move.appliedAtUnixMs, nowMs)}`
    : `sent ${relativeTime(move.sentAtUnixMs, nowMs)}`;
  return `${what}: ${who}, ${since}.`;
}

/** What a late move means, and what the owner can do. */
export const LATE_TEXT =
  "This move has been on its way for a long time: the hub may not be crediting or paying it. Your node waits, and never pays it again. If you are sure it will not arrive, give it up: your node then forgets it and books nothing, and moves nothing by itself until you move funds or turn its own moves on again.";

/** The move the owner gave up, while it holds the node's moves back. */
export function givenUpText(move: HubMove, currency: Currency): string {
  const which = move.direction === Direction.DEPOSIT ? "deposit" : "withdrawal";
  return `You gave up a ${which} of ${
    amount(move.amount, currency)
  }. The hub may still show it; your node then books it, once. Until you move funds or turn its own moves on again, your node moves nothing by itself.`;
}

/** Whether the node moves funds by itself now, for people: between the
 * floor and the ceiling, what is beyond going to the owner's own account
 * (spec/decisions/one-balance-per-asset.md). */
export function automaticText(
  status: Pick<HubStatus, "automatic" | "pausedUntilUnixMs">,
  nowMs: number,
): string {
  if (!status.automatic) {
    return "Your node moves nothing by itself. You move funds yourself.";
  }
  const keeps = "between its floor and ceiling";
  const rest = " What is beyond goes to your own account.";
  const left = status.pausedUntilUnixMs === undefined
    ? 0
    : Number(status.pausedUntilUnixMs) - nowMs;
  if (left >= 1_000) {
    return `Your last move pauses your node's own moves for ${
      durationText(left)
    } more. Then it keeps this account ${keeps} by itself again.${rest}`;
  }
  return `Your node keeps this account ${keeps} by itself.${rest}`;
}

/** A duration of at least a second, for people, rounded up to the unit it
 * is shown in, so what is left never reads as nothing. */
export function durationText(ms: number): string {
  const seconds = Math.ceil(ms / 1_000);
  if (seconds < 60) return `${seconds} s`;
  if (seconds < 3_600) return `${Math.ceil(seconds / 60)} min`;
  if (seconds < 86_400) return `${Math.ceil(seconds / 3_600)} h`;
  return `${Math.ceil(seconds / 86_400)} d`;
}

/** How long the page waits before it sends a withdrawal again while the
 * node checks a token hub's escrow's XRP: a few seconds, about the time a
 * ledger takes to close, so a top-up has applied after a few tries
 * (spec/areas/hubs.md#in-the-node). */
export const GAS_RETRY_MS = 5_000;

/** How many times the page sends a withdrawal again for that, at most:
 * about half a minute in all. */
export const GAS_RETRIES = 6;

/** What the page says while it waits to send a withdrawal again. */
export const GAS_CHECK_TEXT =
  "This hub's escrow pays the XRP Ledger's fees in XRP. Your node is checking that it holds enough, and tops it up first if not. Your withdrawal goes again by itself in a few seconds.";

/** What the page says once it stopped sending a withdrawal again. */
export const GAS_GAVE_UP_TEXT =
  "Your node is still checking that this hub's escrow can pay its fees. Try your withdrawal again in a minute.";

/** A failed call, as the page reads it: the contract's error code, if the
 * node answered with one, and its message. */
export interface CallFailure {
  code?: ErrorCode;
  message: string;
}

/**
 * Whether a refused withdrawal is the node's check of a token hub's
 * escrow's XRP, which ends by itself in a few seconds: RESOURCE_EXHAUSTED,
 * with the node's word that it checks the escrow and to try again shortly
 * (`hub_withdraw_from` in `crates/dassie-node/src/engine/core/hub_moves.rs`,
 * whose tests pin the words). A top-up that failed, which the node checks
 * again only after a backoff of a minute or more, is not, nor is a move
 * on its way.
 */
export function isGasCheck(failure: CallFailure): boolean {
  return failure.code === ErrorCode.RESOURCE_EXHAUSTED &&
    failure.message.includes("escrow pays its fees") &&
    failure.message.includes("try again shortly");
}

/** What the page does after a withdrawal failed, sent again `retries`
 * times already. */
export type WithdrawalNext =
  /** Sends it again after `afterMs`. */
  | { kind: "retry"; afterMs: number }
  /** Stops, and shows `text`. */
  | { kind: "stop"; text: string };

/**
 * After a withdrawal failed with `failure`: while the node checks a token
 * hub's escrow's XRP, send it again in a few seconds, at most
 * {@linkcode GAS_RETRIES} times; stop at any other failure, with the
 * node's own words.
 */
export function afterWithdrawalFailed(
  failure: CallFailure,
  retries: number,
): WithdrawalNext {
  if (!isGasCheck(failure)) return { kind: "stop", text: failure.message };
  if (retries >= GAS_RETRIES) return { kind: "stop", text: GAS_GAVE_UP_TEXT };
  return { kind: "retry", afterMs: GAS_RETRY_MS };
}

/** A hold toward a peer or from one, as the page lists it. */
export interface HoldRow {
  /** The peer's node ID, if the node peers with it over the hub. */
  peer?: string;
  /** The other side's account in the hub. */
  account: number;
  amount: bigint;
}

/** The holds, in the node's order: by peer when there is one. */
export function holdRows(
  holds: readonly { peer: string; account: number; amount: bigint }[],
): HoldRow[] {
  return holds.map((hold) => ({
    peer: hold.peer === "" ? undefined : hold.peer,
    account: hold.account,
    amount: hold.amount,
  }));
}

/** The sum of the holds' amounts. */
export function holdsTotal(holds: readonly { amount: bigint }[]): bigint {
  return holds.reduce((sum, hold) => sum + hold.amount, 0n);
}

/** Where a hub's payout stands, once a validator signed one. */
export type PayoutState =
  /** No validator signed a payout set: the hub runs. */
  | { kind: "running" }
  /** Some signed, not yet a quorum of one set. */
  | { kind: "signing"; signed: number; quorum: number }
  /** A quorum signed one set: its transactions, to submit in order. */
  | {
    kind: "ready";
    height: bigint;
    transactions: readonly string[];
  };

/** Where the payout stands, from what the node keeps of it. */
export function payoutState(payout: GetHubPayoutResponse): PayoutState {
  if (payout.readyHeight !== undefined) {
    return {
      kind: "ready",
      height: payout.readyHeight,
      transactions: payout.transactions,
    };
  }
  if (payout.bundles.length === 0) return { kind: "running" };
  return {
    kind: "signing",
    signed: payout.bundles.length,
    quorum: payout.quorum,
  };
}

/** The payout, for people; nothing while the hub runs. */
export function payoutText(state: PayoutState): string | undefined {
  switch (state.kind) {
    case "running":
      return undefined;
    case "signing":
      return `This hub ended or stalled, and its validators sign its payout to every member: ${state.signed} of the ${state.quorum} it takes have signed so far. Your node keeps what they signed.`;
    case "ready":
      return `This hub ended or stalled, and a quorum of its validators signed its payout, of block ${state.height}: ${state.transactions.length} ${
        state.transactions.length === 1 ? "transaction" : "transactions"
      } to the XRP Ledger that pay every member, you among them. Anyone may submit them, in order, if the validators do not.`;
  }
}

/** `hubs` with `status` in place of the hub of its ID. */
export function withHub(
  hubs: readonly HubStatus[],
  status: HubStatus,
): HubStatus[] {
  return hubs.map((hub) => hub.hubId === status.hubId ? status : hub);
}
