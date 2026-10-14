/**
 * Grants as the owner reads them (D9): who asks, and what for, in plain
 * words: "Send payments of up to $ 10.00 a day".
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
  const parts = interval.split("/");
  const duration = parts.find((part) => part.startsWith("P"));
  if (duration === undefined) return "each period";
  const pieces: string[] = [];
  let time = false;
  for (
    const match of duration.slice(1).matchAll(/(T)|(\d+(?:\.\d+)?)([YMWDHS])/g)
  ) {
    if (match[1] === "T") {
      time = true;
      continue;
    }
    const count = Number(match[2]);
    const unit = time && match[3] === "M" ? "MIN" : match[3];
    const names = unit === "MIN" ? ["minute", "minutes"] : UNITS[unit];
    if (names === undefined) continue;
    pieces.push(`${count} ${count === 1 ? names[0] : names[1]}`);
  }
  if (pieces.length === 1 && pieces[0].startsWith("1 ")) {
    return `a ${pieces[0].slice(2)}`;
  }
  return pieces.length > 0 ? `every ${pieces.join(" and ")}` : "each period";
}

const ACTIONS: Record<string, string> = {
  "outgoing-payment": "Send payments",
  "incoming-payment": "Request payments",
  "quote": "Get quotes for payments",
};

/** What an access item lets the client do, in a sentence. */
export function accessText(access: AccessFields): string {
  const reads = access.actions.some((action) =>
    action === "read-all" || action === "list-all"
  );
  const creates = access.actions.includes("create");
  const base = ACTIONS[access.type] ?? access.type;
  const what = creates ? base : `See ${access.type.replace("-", " ")}s`;
  const limits = access.limits ? limitText(access.limits) : "";
  const scope = reads ? ", and see all of them, not only its own" : "";
  return `${what}${limits}${walletText(access)}${scope}`;
}

/** Which of the owner's wallets it reaches: by the ledger's currency. */
function walletText(access: AccessFields): string {
  const currency = access.ledger.split("+")[1];
  if (currency === undefined || currency === "") return "";
  const from = access.type === "outgoing-payment" ? "from" : "in";
  return ` ${from} your ${currency.toUpperCase()} wallet`;
}

/** The limits of an outgoing-payment grant, after "Send payments". */
export function limitText(limits: LimitFields): string {
  const debit = limits.debitAmount && amountText(limits.debitAmount);
  const receive = limits.receiveAmount && amountText(limits.receiveAmount);
  const every = intervalText(limits.interval);
  let text = debit && receive
    ? ` of up to ${debit}, delivering up to ${receive}`
    : debit || receive
    ? ` of up to ${debit ?? receive}`
    : " of any amount";
  if (every !== undefined) text += ` ${every}`;
  if (limits.receiver !== "") text += `, to one payment request only`;
  return text;
}

/** What it spent in the current window, against its limit, if it has one. */
export function spentText(limits: LimitFields): string | undefined {
  if (limits.debitAmount === undefined) return undefined;
  const spent = amountText(limits.debitAmount, limits.spentDebit);
  const of = amountText(limits.debitAmount);
  return `${spent} of ${of} spent${
    limits.interval === "" ? "" : " this period"
  }`;
}
