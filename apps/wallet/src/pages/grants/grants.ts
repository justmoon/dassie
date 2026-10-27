/**
 * Grants as the owner reads them (spec/decisions/open-payments.md): who asks,
 * and what for, in plain words: "Send payments of up to $ 10.00 a day".
 */

import { currencyOf, formatAmount } from "@dassie/ui/currency";

/** The fields of an access item these helpers read. */
export interface AccessFields {
  type: string;
  actions: readonly string[];
  ledger: string;
  limits?: LimitFields;
}

/** The fields of a grant's limits these helpers read. */
export interface LimitFields {
  receiver: string;
  interval: string;
  debitAmount?: AmountFields;
  receiveAmount?: AmountFields;
  spentDebit: bigint;
  spentReceive: bigint;
  /** When the interval's windows start and end, as the node reads it. */
  startsAtUnixMs?: bigint;
  endsAtUnixMs?: bigint;
  /** The receiver as read while the owner is asked. */
  receiverPayment?: ReceiverFields;
}

/** The receiver a grant may only pay, as the node read it. */
export interface ReceiverFields {
  assetCode: string;
  assetScale: number;
  remaining?: bigint;
  expiresAtUnixMs?: bigint;
  closed: boolean;
  error: string;
  /** Whether the grant's receive limit is in another asset than this. */
  limitInOtherAsset: boolean;
}

/** An amount in an asset. */
export interface AmountFields {
  value: bigint;
  assetCode: string;
  assetScale: number;
}

/** An amount as text: `$ 10.00`. */
export function amountText(amount: AmountFields, value = amount.value): string {
  return formatAmount(value, currencyOf(amount.assetCode, amount.assetScale))
    .text;
}

/** Who asks: its public name, else its wallet address's host and path. */
export function clientName(client: string, publicName: string): string {
  if (publicName !== "") return publicName;
  try {
    const url = new URL(client);
    return `${url.host}${url.pathname === "/" ? "" : url.pathname}`;
  } catch {
    return client;
  }
}

const UNITS: Record<string, [string, string]> = {
  Y: ["year", "years"],
  M: ["month", "months"],
  W: ["week", "weeks"],
  D: ["day", "days"],
  H: ["hour", "hours"],
  S: ["second", "seconds"],
};

/**
 * How often the limit starts afresh, from an ISO 8601 repeating interval:
 * "a day", "every 2 weeks", or none for a limit over all time. A
 * duration of several parts reads as they are, "every 1 month and 10
 * days"; an interval between two dates as "each period".
 */
export function intervalText(interval: string): string | undefined {
  if (interval === "") return undefined;
  const parts = durationParts(interval);
  if (parts === undefined || parts.length === 0) return "each period";
  if (parts.length === 1 && parts[0].count === 1) return `a ${parts[0].one}`;
  return `every ${parts.map(partText).join(" and ")}`;
}

/** One part of a duration: `2 weeks`. */
interface DurationPart {
  count: number;
  one: string;
  many: string;
}

const partText = (part: DurationPart) =>
  `${part.count} ${part.count === 1 ? part.one : part.many}`;

/** The parts of an interval's duration; none between two dates. */
function durationParts(interval: string): DurationPart[] | undefined {
  const duration = interval.split("/").find((part) => part.startsWith("P"));
  if (duration === undefined) return undefined;
  const parts: DurationPart[] = [];
  let time = false;
  for (
    const match of duration.slice(1).matchAll(/(T)|(\d+(?:\.\d+)?)([YMWDHS])/g)
  ) {
    if (match[1] === "T") {
      time = true;
      continue;
    }
    const unit = time && match[3] === "M" ? "MIN" : match[3];
    const names = unit === "MIN" ? ["minute", "minutes"] : UNITS[unit];
    if (names === undefined) continue;
    parts.push({ count: Number(match[2]), one: names[0], many: names[1] });
  }
  return parts;
}

/**
 * How long all the windows of a counted interval last, if its duration
 * has one part: "12 months" for `R11/…/P1M`.
 */
function totalText(interval: string): string | undefined {
  const count = /^R(\d+)\//.exec(interval)?.[1];
  const parts = durationParts(interval);
  if (count === undefined || parts?.length !== 1) return undefined;
  return partText({ ...parts[0], count: parts[0].count * (Number(count) + 1) });
}

const ACTIONS: Record<string, string> = {
  "outgoing-payment": "Send payments",
  "incoming-payment": "Request payments",
  "quote": "Get quotes for payments",
};

/** What an access item lets the client do, in a sentence. */
export function accessText(access: AccessFields): string {
  const creates = access.actions.includes("create");
  const base = ACTIONS[access.type] ?? access.type;
  const what = creates ? base : `See ${access.type.replace("-", " ")}s`;
  // An item that only reads is limited to its receiver, if anything.
  const limits = access.limits === undefined
    ? ""
    : creates
    ? limitText(access.limits)
    : receiverText(access.limits);
  return `${what}${limits}${walletText(access)}${scopeText(access)}`;
}

/**
 * Whether the item reaches others' resources too: one that reads or lists
 * all of a kind sees the owner's own and other apps', and the owner must
 * be told.
 */
function scopeText(access: AccessFields): string {
  const all = access.actions.some((action) =>
    action === "read-all" || action === "list-all"
  );
  return all ? ", and see all of them, not only its own" : "";
}

/** The heading of an item that sends payments, with what else it sees. */
export function paymentsTitle(access: AccessFields): string {
  return `Send payments${scopeText(access)}`;
}

/** Which of the owner's wallets it reaches: by the ledger's currency. */
function walletText(access: AccessFields): string {
  const currency = access.ledger.split("+")[1];
  if (currency === undefined || currency === "") return "";
  const from = access.type === "outgoing-payment" ? "from" : "in";
  return ` ${from} your ${currency.toUpperCase()} wallet`;
}

/**
 * The limits of an outgoing-payment grant, after "Send payments". They
 * limit what is debited, and may also limit what is delivered: what is
 * delivered alone only on grants stored before the node refused that,
 * which still keep to it.
 */
export function limitText(limits: LimitFields): string {
  const debit = limits.debitAmount && amountText(limits.debitAmount);
  const receive = limits.receiveAmount && amountText(limits.receiveAmount);
  const every = intervalText(limits.interval);
  let text = debit && receive
    ? ` of up to ${debit}, delivering up to ${receive}`
    : debit
    ? ` of up to ${debit}`
    : receive
    ? ` delivering up to ${receive}`
    : " of any amount";
  if (every !== undefined) text += ` ${every}`;
  return text + receiverText(limits);
}

/** The receiver the limits keep to, if any. */
function receiverText(limits: LimitFields): string {
  return limits.receiver === "" ? "" : ", to one payment request only";
}

/** What it spent in the current window, against its limits, if it has any. */
export function spentText(limits: LimitFields): string | undefined {
  const period = limits.interval === "" ? "" : " this period";
  const spent = limits.debitAmount &&
    `${amountText(limits.debitAmount, limits.spentDebit)} of ${
      amountText(limits.debitAmount)
    } spent`;
  const delivered = limits.receiveAmount &&
    `${amountText(limits.receiveAmount, limits.spentReceive)} of ${
      amountText(limits.receiveAmount)
    } delivered`;
  const both = [spent, delivered].filter(Boolean).join(", ");
  return both === "" ? undefined : both + period;
}

/*
 * The consent page's facts about an item that sends payments: what the
 * owner sends, what the receiver gets, to whom, and for how long. A grant
 * that caps an amount caps what is debited, and may also cap what is
 * delivered (spec/decisions/open-payments.md), so the debit is firm and what
 * is delivered is said as far as it is known: fees and exchange along the
 * way are only known once a payment is quoted.
 */

/** "a day", or "in total" for a limit over all time. */
function perText(limits: LimitFields): string {
  return intervalText(limits.interval) ?? "in total";
}

/** "every day" for "a day". */
function renewText(interval: string): string {
  const every = intervalText(interval) ?? "each period";
  return every.startsWith("a ") ? `every ${every.slice(2)}` : every;
}

const code = (asset: { assetCode: string }) => asset.assetCode.toUpperCase();

/** What the owner sends. */
export function sendText(access: AccessFields): string {
  const limits = access.limits;
  const wallet = walletText(access);
  if (limits?.debitAmount) {
    return `Up to ${amountText(limits.debitAmount)} ${
      perText(limits)
    }${wallet}, fees and currency exchange included`;
  }
  // Only a request made before the node refused a receive limit alone.
  if (limits?.receiveAmount) {
    const from = wallet === "" ? "" : `,${wallet}`;
    return `No set limit: whatever it costs to deliver the amount below${from}, including fees and currency exchange`;
  }
  return `Any amount${wallet}. The app sets no limit.`;
}

/** What the receiver gets, as far as the grant and its receiver say. */
export function receiveText(limits: LimitFields | undefined): string {
  const capped = limits?.receiveAmount &&
    `Up to ${amountText(limits.receiveAmount)} ${perText(limits)}, in ${
      code(limits.receiveAmount)
    }`;
  const read = limits?.receiverPayment;
  if (read === undefined) {
    return capped ??
      "Depends on who the app pays. Fees and currency exchange along the way come out of what you send.";
  }
  if (read.error !== "") {
    return `Unknown: the payment request could not be read (${read.error})`;
  }
  if (read.closed) {
    return "Nothing: this payment request accepts no more money";
  }
  const owed = read.remaining === undefined
    ? undefined
    : amountText({ ...read, value: read.remaining });
  if (capped !== undefined) {
    return owed === undefined
      ? `${capped}. This payment request has no set amount.`
      : `${capped}. This payment request still asks for ${owed}.`;
  }
  return owed === undefined
    ? `Any amount, in ${code(read)}: this payment request has no set amount`
    : `${owed}, in ${code(read)}: what this payment request still asks for`;
}

/**
 * Why the app can't pay the payment request it names, when the node found
 * the grant's receive limit in another asset than the request: none
 * otherwise. Assets of one code are told apart by their decimal places,
 * which amounts rounded for display would hide.
 */
export function limitAssetText(
  limits: LimitFields | undefined,
): string | undefined {
  const read = limits?.receiverPayment;
  const limit = limits?.receiveAmount;
  if (read?.limitInOtherAsset !== true || limit === undefined) return undefined;
  // The node compares codes as sent, so "usd" and "USD" differ: name them
  // as sent then, since uppercased they would read alike.
  const [mine, theirs] = limit.assetCode === read.assetCode
    ? [
      `counts ${code(limit)} to ${placesText(limit.assetScale)}`,
      `counts it to ${placesText(read.assetScale)}`,
    ]
    : code(limit) === code(read)
    ? [`is in "${limit.assetCode}"`, `is paid in "${read.assetCode}"`]
    : [`is in ${code(limit)}`, `is paid in ${code(read)}`];
  return `The app can't pay this payment request: the limit ${mine}, and the request ${theirs}. Every payment it tries will be refused.`;
}

const placesText = (scale: number) =>
  `${scale} decimal ${scale === 1 ? "place" : "places"}`;

/** To whom: anyone, or the one payment request. */
export function toText(
  limits: LimitFields | undefined,
  timeZone?: string,
): string {
  if (limits === undefined || limits.receiver === "") {
    return "Anyone the app chooses";
  }
  const read = limits.receiverPayment;
  const open = read?.expiresAtUnixMs !== undefined && read.error === "" &&
      !read.closed
    ? `, open until ${dateText(read.expiresAtUnixMs, timeZone)}`
    : "";
  return `One payment request at ${hostOf(limits.receiver)}${open}`;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** How long the grant lasts, from the span the node read of its interval. */
export function spanText(
  limits: LimitFields | undefined,
  nowMs: bigint,
  timeZone?: string,
): string {
  const date = (ms: bigint) => dateText(ms, timeZone);
  if (limits === undefined || limits.interval === "") {
    const total = limits?.debitAmount ?? limits?.receiveAmount;
    const once = total && ` The ${amountText(total)} is a total, not renewed.`;
    if (limits !== undefined && limits.receiver !== "") {
      return `Until the payment request is paid or closes.${once ?? ""}`;
    }
    return total === undefined
      ? "No end date, until you revoke access."
      : `No end date. The ${
        amountText(total)
      } is a total for all payments, not renewed, until you revoke access.`;
  }
  const renews = `The limit renews ${renewText(limits.interval)}`;
  const { startsAtUnixMs: start, endsAtUnixMs: end } = limits;
  if (end !== undefined && end <= nowMs) {
    return `Ended on ${date(end)}: no payment can be made.`;
  }
  const later = start !== undefined && start > nowMs;
  const before = later ? "; nothing can be sent before then" : "";
  if (end === undefined) {
    const from = later ? `Starts ${date(start)}${before}. ` : "";
    return `${from}No end date. ${renews} until you revoke access.`;
  }
  const total = totalText(limits.interval);
  // `R0` is the first window only: nothing renews.
  const after = /^R0\//.test(limits.interval) ? "" : ` ${renews}.`;
  if (later) {
    const length = total === undefined ? "" : ` (${total})`;
    return `From ${date(start)} until ${date(end)}${length}${before}.${after}`;
  }
  const since = start === undefined
    ? ""
    : ` (${total === undefined ? "" : `${total} `}from ${date(start)})`;
  return `Until ${date(end)}${since}.${after}`;
}

/**
 * A date for people, in English like the rest of the wallet, with the time
 * only when it is not midnight: "Oct 16, 2026, 6:30 PM". In the owner's
 * time zone, unless one is given.
 */
export function dateText(ms: bigint, timeZone?: string): string {
  const date = new Date(Number(ms));
  const day = date.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone,
  });
  const clock = (options: Intl.DateTimeFormatOptions) =>
    date.toLocaleTimeString("en-US", { ...options, timeZone })
      // ICU puts a narrow space before "PM".
      .replace(/\s/gu, " ");
  const midnight = clock({
    hourCycle: "h23",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }) === "00:00:00";
  return midnight
    ? day
    : `${day}, ${clock({ hour: "numeric", minute: "2-digit" })}`;
}
