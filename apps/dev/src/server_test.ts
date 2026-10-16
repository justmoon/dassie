import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { refusal } from "./server.ts";

test("only the dashboard's own page reaches the owner's side", () => {
  assertEquals(
    refusal("localhost:10100", "https://localhost:10100"),
    undefined,
  );
  assertEquals(refusal("localhost", "https://localhost"), undefined);
  assertEquals(
    refusal("dev-rpc.localhost:10101", "https://dev-rpc.localhost:10101"),
    undefined,
  );
  // Another page.
  assertEquals(
    refusal("localhost:10100", "https://evil.example"),
    "not the dashboard's page",
  );
  assertEquals(refusal("localhost:10100", null), "not the dashboard's page");
  // The same page at another port is another origin.
  assertEquals(
    refusal("localhost:10100", "https://localhost:10101"),
    "not the dashboard's page",
  );
  // A rebound name, whatever its origin says.
  assertEquals(
    refusal("evil.example:10100", "https://evil.example:10100"),
    "not a name of the dashboard",
  );
  assertEquals(
    refusal("d1.localhost:10100", "https://d1.localhost:10100"),
    "not a name of the dashboard",
  );
  assertEquals(
    refusal(null, "https://localhost"),
    "not a name of the dashboard",
  );
});
