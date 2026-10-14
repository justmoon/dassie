import { assertEquals } from "@std/assert";
import { FakeTime } from "@std/testing/time";
import { ErrorCode, type ItemFor, RpcError } from "@dassie/rpc";
import {
  type LiveStatus,
  type StreamClient,
  subscribe,
} from "./subscription.ts";

type Logs = ItemFor<"subscribeLogs">;

/** A client whose streams the test ends by hand. */
class FakeClient implements StreamClient {
  streams: {
    onItem: (item: Logs) => void;
    resolve: () => void;
    reject: (error: unknown) => void;
    signal?: AbortSignal;
  }[] = [];

  // deno-lint-ignore no-explicit-any
  stream(_method: any, _request: any, onItem: any, options?: any) {
    return new Promise<void>((resolve, reject) => {
      this.streams.push({ onItem, resolve, reject, signal: options?.signal });
    });
  }
}

const flush = () => new Promise((resolve) => queueMicrotask(() => resolve(0)));

Deno.test("a stream that fails is subscribed again with a growing backoff", async () => {
  using time = new FakeTime();
  const client = new FakeClient();
  const statuses: LiveStatus[] = [];
  const items: number[] = [];
  const sub = subscribe(client, "subscribeLogs", {}, {
    onItem: (_item, generation) => items.push(generation),
    onStatus: (status) => statuses.push(status),
  }, { random: () => 0.5, backoffMinMs: 100 });
  await flush();
  client.streams[0].onItem({} as Logs);
  await flush();
  client.streams[0].reject(new RpcError(ErrorCode.UNAVAILABLE, "restart"));
  await flush();
  await flush();
  time.tick(99);
  assertEquals(client.streams.length, 1);
  time.tick(1);
  assertEquals(client.streams.length, 2, "again after 100 ms");
  client.streams[1].reject(new RpcError(ErrorCode.UNAVAILABLE, "restart"));
  await flush();
  await flush();
  time.tick(199);
  assertEquals(client.streams.length, 2);
  time.tick(1);
  assertEquals(client.streams.length, 3, "then after 200 ms");
  client.streams[2].onItem({} as Logs);
  assertEquals(items, [1, 3], "each attempt is a generation");
  await flush();
  sub.stop();
  assertEquals(client.streams[2].signal?.aborted, true);
  assertEquals(statuses.map((s) => s.kind), [
    "connecting",
    "live",
    "retrying",
    "connecting",
    "retrying",
    "connecting",
    "live",
  ]);
});

Deno.test("errors no attempt can fix end the subscription", async () => {
  using time = new FakeTime();
  const client = new FakeClient();
  const statuses: LiveStatus[] = [];
  subscribe(client, "subscribeLogs", {}, {
    onItem: () => {},
    onStatus: (status) => statuses.push(status),
  });
  client.streams[0].reject(
    new RpcError(ErrorCode.UNIMPLEMENTED, "unknown method"),
  );
  await flush();
  await flush();
  time.tick(60_000);
  assertEquals(client.streams.length, 1);
  assertEquals(statuses.at(-1)?.kind, "failed");
});
