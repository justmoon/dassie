import { assertEquals } from "@std/assert";
import {
  destinationLabel,
  lapseText,
  looksLikeRecipient,
  progress,
  quoteAmount,
  recipientName,
  stateText,
} from "./send.ts";

Deno.test("the amount of a quote is what is fixed, or none", () => {
  assertEquals(quoteAmount("receive", 5n), {
    case: "receiveAmount",
    value: 5n,
  });
  assertEquals(quoteAmount("send", 5n), { case: "debitAmount", value: 5n });
  assertEquals(quoteAmount("send", undefined), { case: undefined });
});

Deno.test("a recipient is named by its name and pointer, or its pointer", () => {
  const address = "https://wallet.example/.well-known/pay";
  assertEquals(
    recipientName({
      publicName: "Alice",
      paymentPointer: "$wallet.example",
      walletAddress: address,
    }),
    "Alice ($wallet.example)",
  );
  assertEquals(
    recipientName({
      publicName: "",
      paymentPointer: "$d2.localhost:5002",
      walletAddress: address,
    }),
    "$d2.localhost:5002",
  );
  // No pointer names it: its URL.
  assertEquals(
    recipientName({
      publicName: "",
      paymentPointer: "",
      walletAddress: "https://wallet.example",
    }),
    "https://wallet.example",
  );
});

Deno.test("a payment goes to its pointer, or else its receiver's host", () => {
  assertEquals(
    destinationLabel({
      recipientPointer: "$d2.localhost",
      receiver: "https://d2.localhost/incoming-payments/1",
    }),
    "$d2.localhost",
  );
  assertEquals(
    destinationLabel({
      recipientPointer: "",
      receiver: "https://wallet.example:8443/incoming-payments/1",
    }),
    "wallet.example:8443",
  );
  assertEquals(
    destinationLabel({ recipientPointer: "", receiver: "not a url" }),
    "not a url",
  );
});

Deno.test("pointers and HTTPS URLs look like recipients", () => {
  for (
    const input of [
      "$d2.localhost",
      " $wallet.example/alice ",
      "https://w.example/pay",
    ]
  ) {
    assertEquals(looksLikeRecipient(input), true, input);
  }
  for (const input of ["", "$", "alice", "http://w.example", "$ spaced"]) {
    assertEquals(looksLikeRecipient(input), false, input);
  }
});

Deno.test("progress is in thousandths, and never past the end", () => {
  assertEquals(progress(0n, 100n), 0);
  assertEquals(progress(25n, 100n), 250);
  assertEquals(progress(1n, 3n), 333);
  assertEquals(progress(200n, 100n), 1000);
  assertEquals(progress(5n, 0n), 0);
});

Deno.test("a quote says how long it is good for", () => {
  assertEquals(lapseText(10_000n, 10_000), "This quote has lapsed.");
  assertEquals(lapseText(10_000n, 1_000), "Good for 9 more seconds.");
  assertEquals(lapseText(70_000n, 0), "Good for 2 more minutes.");
  assertEquals(lapseText(60_000n, 0), "Good for 1 more minute.");
});

Deno.test("states read as words", () => {
  assertEquals(stateText("sending"), "Sending…");
  assertEquals(stateText("completed"), "Sent");
  assertEquals(stateText("failed"), "Failed");
});
