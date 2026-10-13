import { assertEquals } from "@std/assert";
import { contractHash } from "./scripts/hash.ts";

// The generated code must follow the contract: after changing a .proto
// file, run `deno task gen` in packages/rpc.
Deno.test("src/gen matches proto/dassie/ui/v1", async () => {
  const recorded = await Deno.readTextFile(
    new URL("./src/gen/dassie/ui/v1/contract.sha256", import.meta.url),
  );
  assertEquals(
    recorded.trim(),
    await contractHash(),
    "run `deno task gen` in packages/rpc",
  );
});
