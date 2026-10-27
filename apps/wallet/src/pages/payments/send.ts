/**
 * What the Send and payment status pages say, apart from the pages: whom a
 * payment goes to, how far it got, and how long a quote is good for.
 */

/** The steps of a Send. */
export type Step = "recipient" | "amount" | "review";

/** Which amount the owner fixes: what the recipient gets, or what they
 * send. */
export type Fixed = "receive" | "send";

/** The amount of a quote request: none for what a request still asks. */
export function quoteAmount(
  fixed: Fixed,
  value: bigint | undefined,
):
  | { case: "receiveAmount"; value: bigint }
  | { case: "debitAmount"; value: bigint }
  | { case: undefined } {
  if (value === undefined) return { case: undefined };
  return fixed === "send"
    ? { case: "debitAmount", value }
    : { case: "receiveAmount", value };
}

/** Whom a recipient is, in a few words: its name and payment pointer,
 * or its wallet address where no pointer names it. */
export function recipientName(
  recipient: {
    publicName: string;
    paymentPointer: string;
    walletAddress: string;
  },
): string {
  const address = recipient.paymentPointer !== ""
    ? recipient.paymentPointer
    : recipient.walletAddress;
  return recipient.publicName !== ""
    ? `${recipient.publicName} (${address})`
    : address;
}

/** Where an outgoing payment goes: the payment pointer the owner named,
 * or else the host of the incoming payment it pays. */
export function destinationLabel(
  payment: { recipientPointer: string; receiver: string },
): string {
  if (payment.recipientPointer !== "") return payment.recipientPointer;
  try {
    return new URL(payment.receiver).host;
  } catch {
    return payment.receiver;
  }
}

/** Whether the input could name a recipient: a payment pointer or an
 * HTTPS URL. The node has the last word. */
export function looksLikeRecipient(input: string): boolean {
  const text = input.trim();
  return /^\$[^\s$/]+/.test(text) || /^https:\/\/[^\s/]+/.test(text);
}

/** How far a payment got, in thousandths of its debit amount. */
export function progress(sent: bigint, debit: bigint): number {
  if (debit <= 0n) return 0;
  const permille = (sent * 1000n) / debit;
  return Number(permille > 1000n ? 1000n : permille);
}

/** How long a quote is still good for, or that it lapsed. */
export function lapseText(expiresAtUnixMs: bigint, nowMs: number): string {
  const left = Number(expiresAtUnixMs) - nowMs;
  if (left <= 0) return "This quote has lapsed.";
  const seconds = Math.ceil(left / 1000);
  if (seconds < 60) return `Good for ${seconds} more seconds.`;
  const minutes = Math.ceil(seconds / 60);
  return `Good for ${minutes} more minute${minutes === 1 ? "" : "s"}.`;
}

/** What a payment's state means, for people. */
export function stateText(state: string): string {
  switch (state) {
    case "sending":
      return "Sending…";
    case "completed":
      return "Sent";
    case "failed":
      return "Failed";
    default:
      return state;
  }
}
