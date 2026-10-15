import { assertEquals } from "@std/assert";
import { DEFAULT_FORM, type Form, parseForm } from "./form.ts";
import { DEFAULT_SETTINGS } from "./simulation.ts";

Deno.test("the default form is the original's defaults", () => {
  assertEquals(parseForm(DEFAULT_FORM), {
    settings: DEFAULT_SETTINGS,
    timeDilation: 1,
  });
});

Deno.test("the form takes spaces and decimals where they make sense", () => {
  const parsed = parseForm({
    ...DEFAULT_FORM,
    amount: " 18446744073709551615 ",
    jitter: "0.5",
    timeDilation: "0.1",
  });
  assertEquals(typeof parsed, "object");
  if (typeof parsed === "string") return;
  assertEquals(parsed.settings.amount, 0xffff_ffff_ffff_ffffn);
  assertEquals(parsed.settings.jitter, 0.5);
  assertEquals(parsed.timeDilation, 0.1);
});

Deno.test("the form says what is wrong", () => {
  const cases: [Partial<Form>, string][] = [
    [{ amount: "" }, "The amount must be a whole number."],
    [{ amount: "1.5" }, "The amount must be a whole number."],
    [{ amount: "-3" }, "The amount must be a whole number."],
    [{ amount: "0" }, "The amount must be between 1 and 2⁶⁴ − 1."],
    [
      { amount: "18446744073709551616" },
      "The amount must be between 1 and 2⁶⁴ − 1.",
    ],
    [{ maxPacketAmount: "x" }, "The largest packet must be a whole number."],
    [{ latency: "" }, "The latency must be a number."],
    [{ latency: "Infinity" }, "The latency must be a number."],
    [{ jitter: "5" }, "The jitter must be between 0 and 2."],
    [
      { maxPacketsInFlight: "2.5" },
      "Packets in flight must be a whole number from 1 to 1000.",
    ],
    [{ seed: "-1" }, "The seed must be a whole number from 0 to 4294967295."],
    [{ timeDilation: "0" }, "The time dilation must be between 0.01 and 100."],
  ];
  for (const [changes, message] of cases) {
    assertEquals(parseForm({ ...DEFAULT_FORM, ...changes }), message);
  }
});
