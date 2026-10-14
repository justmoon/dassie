import { assertEquals } from "@std/assert";
import { errorExcerpt, MAX_EXCERPT_BYTES } from "./build.ts";

Deno.test("a failed build's excerpt starts at its first error", () => {
  const output = [
    "   Compiling dassie-node v0.1.0",
    "warning: unused import",
    "error[E0425]: cannot find value `x` in this scope",
    "  --> crates/dassie-node/src/lib.rs:1:1",
    "error: could not compile `dassie-node`",
  ].join("\n");
  assertEquals(
    errorExcerpt(output).split("\n")[0],
    "error[E0425]: cannot find value `x` in this scope",
  );
  assertEquals(errorExcerpt("no errors here"), "no errors here");
  const long = `error: ${"x".repeat(MAX_EXCERPT_BYTES * 2)}`;
  assertEquals(errorExcerpt(long).length, MAX_EXCERPT_BYTES + 1);
});
