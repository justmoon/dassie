import { describe, it } from "node:test";
import { assertEquals } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import { GetSwapStatusResponseSchema } from "@dassie/rpc/proto";
import { swapsView } from "./swaps.ts";

describe("swapsView", () => {
  it("waits for the node", () => {
    assertEquals(swapsView(undefined, false), undefined);
  });

  it("says a core node swaps by itself, and what it did", () => {
    const status = create(GetSwapStatusResponseSchema, {
      byItself: true,
      peers: 1n,
      sent: 12n,
      fulfilled: 9n,
      rejected: 3n,
      backoffs: 2n,
      refused: 1n,
    });
    const view = swapsView(status, false)!;
    assertEquals(view.badge, "swapping");
    assertEquals(view.tone, "good");
    assertEquals(
      view.text,
      "Your node swaps with its peers toward an even split of what it holds, by the quotes of 1 peer.",
    );
    assertEquals(view.details, [
      "Sent 12 swaps: 9 fulfilled, 3 rejected, 0 in flight.",
      "Backed off 2 times; refused 1 time; 0 came back short, 0 too large.",
    ]);
  });

  it("says a periphery node without targets swaps only when asked", () => {
    const status = create(GetSwapStatusResponseSchema, { peers: 2n });
    const view = swapsView(status, true)!;
    assertEquals(view.badge, "on request");
    assertEquals(view.tone, "plain");
    assertEquals(
      view.text,
      "Your node swaps only when you ask: set targets in Settings to have it swap by itself.",
    );
    assertEquals(view.details, []);
  });

  it("names a swap that paid though rejected, and dropped outcomes", () => {
    const status = create(GetSwapStatusResponseSchema, {
      byItself: true,
      sent: 1n,
      rejected: 1n,
      paidThoughRejected: 1n,
      dropped: 2n,
    });
    const view = swapsView(status, false)!;
    assertEquals(view.tone, "bad");
    assertEquals(view.details.slice(2), [
      "1 swap rejected after paying your node all the same.",
      "2 outcomes dropped unread.",
    ]);
  });
});
