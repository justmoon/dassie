import { test } from "node:test";
import { assertEquals } from "@std/assert";
import {
  accessText,
  clientName,
  dateText,
  intervalText,
  limitAssetText,
  type LimitFields,
  limitText,
  paymentsTitle,
  receiveText,
  sendText,
  spanText,
  spentText,
  toText,
} from "./grants.ts";

const usd = (value: bigint) => ({ value, assetCode: "USD", assetScale: 9 });

test("intervals read as people say them", () => {
  assertEquals(intervalText(""), undefined);
  assertEquals(intervalText("R/2024-01-01T00:00:00Z/P1D"), "a day");
  assertEquals(intervalText("R12/2024-01-01T00:00:00Z/P1M"), "a month");
  assertEquals(intervalText("R/2024-01-01T00:00:00Z/P2W"), "every 2 weeks");
  assertEquals(
    intervalText("R-1/P1M10DT2H30M/2024-01-01T00:00:00Z"),
    "every 1 month and 10 days and 2 hours and 30 minutes",
  );
  assertEquals(
    intervalText("R/2024-01-01T00:00:00Z/2024-02-01T00:00:00Z"),
    "each period",
  );
});

test("limits read as a sentence", () => {
  const limits = {
    receiver: "",
    interval: "R/2024-01-01T00:00:00Z/P1D",
    debitAmount: usd(10_000_000_000n),
    spentDebit: 2_500_000_000n,
    spentReceive: 0n,
  };
  assertEquals(limitText(limits), " of up to $ 10.00 a day");
  assertEquals(
    accessText({
      type: "outgoing-payment",
      actions: ["create", "read"],
      ledger: "stub+usd",
      limits,
    }),
    "Send payments of up to $ 10.00 a day from your USD wallet",
  );
  assertEquals(spentText(limits), "$ 2.50 of $ 10.00 spent this period");
  assertEquals(
    limitText({ ...limits, interval: "", debitAmount: undefined }),
    " of any amount",
  );
});

test("a limit on what is delivered says so", () => {
  const limits = {
    receiver: "",
    interval: "R/2024-01-01T00:00:00Z/P1D",
    debitAmount: usd(10_000_000_000n),
    receiveAmount: usd(5_000_000_000n),
    spentDebit: 1_500_000_000n,
    spentReceive: 1_000_000_000n,
  };
  assertEquals(
    limitText(limits),
    " of up to $ 10.00, delivering up to $ 5.00 a day",
  );
  assertEquals(
    spentText(limits),
    "$ 1.50 of $ 10.00 spent, $ 1.00 of $ 5.00 delivered this period",
  );
  // Grants stored before the node refused a receive limit alone still
  // show it.
  const alone = { ...limits, interval: "", debitAmount: undefined };
  assertEquals(limitText(alone), " delivering up to $ 5.00");
  assertEquals(spentText(alone), "$ 1.00 of $ 5.00 delivered");
});

test("an item that only reads shows only its receiver", () => {
  const limits = {
    receiver: "https://example.com/incoming-payments/1",
    interval: "",
    spentDebit: 0n,
    spentReceive: 0n,
  };
  assertEquals(
    accessText({
      type: "outgoing-payment",
      actions: ["read"],
      ledger: "stub+usd",
      limits,
    }),
    "See outgoing payments, to one payment request only from your USD wallet",
  );
});

test("reading all of a kind says so", () => {
  assertEquals(
    accessText({ type: "quote", actions: ["read-all"], ledger: "" }),
    "See quotes, and see all of them, not only its own",
  );
});

test("a client goes by its public name, else its address", () => {
  assertEquals(
    clientName("https://app.example/me", "Budget App"),
    "Budget App",
  );
  assertEquals(clientName("https://app.example/me", ""), "app.example/me");
});

// The consent page's facts, for each kind of grant (Sabine Schaller's
// review): what is sent, what is received, to whom, and for how long. The
// owner's wallet is in USD; dates read in UTC, on 2026-10-16 at noon.

const eur = (value: bigint) => ({ value, assetCode: "EUR", assetScale: 2 });
const at = (iso: string) => BigInt(Date.parse(iso));
const NOW = at("2026-10-16T12:00:00Z");
const SHOP = "https://shop.example/incoming-payments/abc";

/** What the consent page shows of an item with these limits. */
function facts(limits: LimitFields) {
  return {
    send: sendText({
      type: "outgoing-payment",
      actions: ["create", "read"],
      ledger: "stub+usd",
      limits,
    }),
    receive: receiveText(limits),
    to: toText(limits, "UTC"),
    lasts: spanText(limits, NOW, "UTC"),
  };
}

const none: LimitFields = {
  receiver: "",
  interval: "",
  spentDebit: 0n,
  spentReceive: 0n,
};
const depends =
  "Depends on who the app pays. Fees and currency exchange along the way come out of what you send.";
const noLimit =
  "No set limit: whatever it costs to deliver the amount below, from your USD wallet, including fees and currency exchange";
const shopRead = {
  assetCode: "EUR",
  assetScale: 2,
  remaining: 2000n,
  expiresAtMs: at("2026-10-16T18:30:00Z"),
  closed: false,
  error: "",
  limitInOtherAsset: false,
};

test("A: a fixed send, renewed for ever", () => {
  assertEquals(
    facts({
      ...none,
      interval: "R/2026-10-16T08:00:00Z/P1D",
      debitAmount: usd(1_000_000_000n),
      startsAtMs: at("2026-10-16T08:00:00Z"),
    }),
    {
      send:
        "Up to $ 1.00 a day from your USD wallet, fees and currency exchange included",
      receive: depends,
      to: "Anyone the app chooses",
      lasts: "No end date. The limit renews every day until you revoke access.",
    },
  );
});

test("B: a capped receive in another currency, with an end", () => {
  const limits = {
    ...none,
    interval: "R11/2026-10-01T00:00:00Z/P1M",
    debitAmount: usd(6_000_000_000n),
    receiveAmount: eur(500n),
    startsAtMs: at("2026-10-01T00:00:00Z"),
    endsAtMs: at("2027-10-01T00:00:00Z"),
  };
  assertEquals(facts(limits), {
    send:
      "Up to $ 6.00 a month from your USD wallet, fees and currency exchange included",
    receive: "Up to € 5.00 a month, in EUR",
    to: "Anyone the app chooses",
    lasts:
      "Until Oct 1, 2027 (12 months from Oct 1, 2026). The limit renews every month.",
  });
  // A request made before the node refused a receive limit alone.
  assertEquals(
    facts({ ...limits, debitAmount: undefined }).send,
    noLimit,
  );
});

test("C: a capped receive, once, to one payment request", () => {
  assertEquals(
    facts({
      ...none,
      receiver: SHOP,
      debitAmount: usd(25_000_000_000n),
      receiveAmount: eur(2000n),
      receiverPayment: shopRead,
    }),
    {
      send:
        "Up to $ 25.00 in total from your USD wallet, fees and currency exchange included",
      receive:
        "Up to € 20.00 in total, in EUR. This payment request still asks for € 20.00.",
      to:
        "One payment request at shop.example, open until Oct 16, 2026, 6:30 PM",
      lasts:
        "Until the payment request is paid or closes. The $ 25.00 is a total, not renewed.",
    },
  );
});

test("D: a fixed send, once, to one payment request in another currency", () => {
  const limits = {
    ...none,
    receiver: SHOP,
    debitAmount: usd(25_000_000_000n),
    receiverPayment: shopRead,
  };
  assertEquals(facts(limits), {
    send:
      "Up to $ 25.00 in total from your USD wallet, fees and currency exchange included",
    receive: "€ 20.00, in EUR: what this payment request still asks for",
    to: "One payment request at shop.example, open until Oct 16, 2026, 6:30 PM",
    lasts:
      "Until the payment request is paid or closes. The $ 25.00 is a total, not renewed.",
  });
  assertEquals(
    receiveText({
      ...limits,
      receiverPayment: { ...shopRead, remaining: undefined },
    }),
    "Any amount, in EUR: this payment request has no set amount",
  );
  assertEquals(
    receiveText({ ...limits, receiverPayment: { ...shopRead, closed: true } }),
    "Nothing: this payment request accepts no more money",
  );
  const unread = {
    ...limits,
    receiverPayment: {
      assetCode: "",
      assetScale: 0,
      closed: false,
      error: "the receiver cannot be read: unreachable",
      limitInOtherAsset: false,
    },
  };
  assertEquals(
    receiveText(unread),
    "Unknown: the payment request could not be read (the receiver cannot be read: unreachable)",
  );
  assertEquals(toText(unread, "UTC"), "One payment request at shop.example");
});

test("a receive limit in another asset than its payment request is flagged", () => {
  const limits = {
    ...none,
    receiver: SHOP,
    receiveAmount: usd(5_000_000_000n),
    receiverPayment: { ...shopRead, limitInOtherAsset: true },
  };
  assertEquals(
    limitAssetText(limits),
    "The app can't pay this payment request: the limit is in USD, and the request is paid in EUR. Every payment it tries will be refused.",
  );
  // One code at two scales would read "USD" twice: the places tell them apart.
  assertEquals(
    limitAssetText({
      ...limits,
      receiveAmount: { value: 500n, assetCode: "USD", assetScale: 2 },
      receiverPayment: {
        ...limits.receiverPayment,
        assetCode: "USD",
        assetScale: 9,
      },
    }),
    "The app can't pay this payment request: the limit counts USD to 2 decimal places, and the request counts it to 9 decimal places. Every payment it tries will be refused.",
  );
  // Codes are compared as sent: "usd" is not "USD", and reads as sent.
  assertEquals(
    limitAssetText({
      ...limits,
      receiveAmount: { value: 500n, assetCode: "usd", assetScale: 9 },
      receiverPayment: {
        ...limits.receiverPayment,
        assetCode: "USD",
        assetScale: 9,
      },
    }),
    'The app can\'t pay this payment request: the limit is in "usd", and the request is paid in "USD". Every payment it tries will be refused.',
  );
  // The node decides; the page says nothing unless it found a mismatch.
  assertEquals(
    limitAssetText({ ...limits, receiverPayment: shopRead }),
    undefined,
  );
  assertEquals(
    limitAssetText({ ...none, receiveAmount: eur(500n) }),
    undefined,
  );
  assertEquals(limitAssetText(undefined), undefined);
});

test("E: a total for all payments, to anyone", () => {
  const shown = facts({ ...none, debitAmount: usd(50_000_000_000n) });
  assertEquals(
    shown.send,
    "Up to $ 50.00 in total from your USD wallet, fees and currency exchange included",
  );
  assertEquals(
    shown.lasts,
    "No end date. The $ 50.00 is a total for all payments, not renewed, until you revoke access.",
  );
});

test("F: a start in the future, renewed for ever", () => {
  const shown = facts({
    ...none,
    interval: "R/2026-11-01T00:00:00Z/P1M",
    debitAmount: usd(20_000_000_000n),
    startsAtMs: at("2026-11-01T00:00:00Z"),
  });
  assertEquals(
    shown.send,
    "Up to $ 20.00 a month from your USD wallet, fees and currency exchange included",
  );
  assertEquals(
    shown.lasts,
    "Starts Nov 1, 2026; nothing can be sent before then. No end date. The limit renews every month until you revoke access.",
  );
});

test("G: a start in the future, with an end", () => {
  assertEquals(
    facts({
      ...none,
      interval: "R2/2026-11-01T00:00:00Z/P1M",
      debitAmount: usd(20_000_000_000n),
      startsAtMs: at("2026-11-01T00:00:00Z"),
      endsAtMs: at("2027-02-01T00:00:00Z"),
    }).lasts,
    "From Nov 1, 2026 until Feb 1, 2027 (3 months); nothing can be sent before then. The limit renews every month.",
  );
});

test("H: an end only", () => {
  assertEquals(
    facts({
      ...none,
      interval: "R/P1M/2027-01-01T00:00:00Z",
      debitAmount: usd(20_000_000_000n),
      endsAtMs: at("2027-01-01T00:00:00Z"),
    }).lasts,
    "Until Jan 1, 2027. The limit renews every month.",
  );
});

test("I: no limits at all", () => {
  assertEquals(facts(none), {
    send: "Any amount from your USD wallet. The app sets no limit.",
    receive: depends,
    to: "Anyone the app chooses",
    lasts: "No end date, until you revoke access.",
  });
});

// Found in review: the payments block must still say when the app also
// sees every outgoing payment of the wallet, not only its own.
test("sending payments says when the app sees all of them", () => {
  const item = { type: "outgoing-payment", ledger: "stub+usd", limits: none };
  assertEquals(
    paymentsTitle({ ...item, actions: ["create", "read"] }),
    "Send payments",
  );
  assertEquals(
    paymentsTitle({ ...item, actions: ["create", "read-all"] }),
    "Send payments, and see all of them, not only its own",
  );
  assertEquals(
    paymentsTitle({ ...item, actions: ["create", "list-all"] }),
    "Send payments, and see all of them, not only its own",
  );
});

// Found in review: `R0` is one window, which does not renew.
test("a single window does not renew", () => {
  const once = {
    ...none,
    interval: "R0/2026-10-01T00:00:00Z/P1M",
    debitAmount: usd(1_000_000_000n),
    startsAtMs: at("2026-10-01T00:00:00Z"),
    endsAtMs: at("2026-11-01T00:00:00Z"),
  };
  assertEquals(
    facts(once).lasts,
    "Until Nov 1, 2026 (1 month from Oct 1, 2026).",
  );
  assertEquals(
    facts({
      ...once,
      interval: "R0/2026-11-01T00:00:00Z/P1M",
      startsAtMs: at("2026-11-01T00:00:00Z"),
      endsAtMs: at("2026-12-01T00:00:00Z"),
    }).lasts,
    "From Nov 1, 2026 until Dec 1, 2026 (1 month); nothing can be sent before then.",
  );
});

test("an interval that has ended says nothing can be sent", () => {
  assertEquals(
    facts({
      ...none,
      interval: "R0/2026-09-01T00:00:00Z/P1M",
      debitAmount: usd(1_000_000_000n),
      startsAtMs: at("2026-09-01T00:00:00Z"),
      endsAtMs: at("2026-10-01T00:00:00Z"),
    }).lasts,
    "Ended on Oct 1, 2026: no payment can be made.",
  );
});

test("dates show their time only when it is not midnight", () => {
  assertEquals(dateText(at("2026-11-01T00:00:00Z"), "UTC"), "Nov 1, 2026");
  assertEquals(
    dateText(at("2026-11-01T09:05:00Z"), "UTC"),
    "Nov 1, 2026, 9:05 AM",
  );
  // Midnight where the owner is, not in UTC.
  assertEquals(
    dateText(at("2026-10-31T23:00:00Z"), "Europe/Berlin"),
    "Nov 1, 2026",
  );
});
