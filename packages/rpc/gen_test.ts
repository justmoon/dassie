import { assertEquals } from "@std/assert";
import { readFile } from "node:fs/promises";
import { contractHash, CONTRACTS } from "./scripts/hash.ts";

// The generated code must follow the contracts: after changing a .proto
// file, run `deno task gen` in packages/rpc.
for (const contract of CONTRACTS) {
  Deno.test(`src/gen matches proto/${contract}`, async () => {
    const recorded = await readFile(
      new URL(`./src/gen/${contract}/contract.sha256`, import.meta.url),
      "utf8",
    );
    assertEquals(
      recorded.trim(),
      await contractHash(contract),
      "run `deno task gen` in packages/rpc",
    );
  });
}
