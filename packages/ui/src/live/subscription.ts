/**
 * Keeps a live stream (spec/areas/rpc.md#live-streams) open: subscribes, and
 * subscribes again with a backoff when the stream ends or fails for a reason
 * that may pass, such as the node's engine restarting. Each attempt is a new
 * generation, whose first item is a fresh snapshot.
 *
 * Nothing here knows Solid: the wallet's live views build on it, and it is
 * tested on its own.
 */

import {
  ErrorCode,
  type ItemFor,
  type RequestFor,
  RpcError,
  type StreamMethod,
  type StreamOptions,
} from "@dassie/rpc";

/** Where a subscription is. */
export type LiveStatus =
  | { kind: "connecting" }
  | { kind: "live" }
  | { kind: "retrying"; message: string; attempt: number }
  | { kind: "failed"; message: string; code: ErrorCode };

/** The client's part a subscription uses. */
export interface StreamClient {
  stream<M extends StreamMethod>(
    method: M,
    request: RequestFor<M>,
    onItem: (item: ItemFor<M>) => void,
    options?: StreamOptions,
  ): Promise<void>;
}

/** A running subscription. */
export interface Subscription {
  stop(): void;
}

/** How a subscription retries. */
export interface RetryOptions {
  backoffMinMs?: number;
  backoffMaxMs?: number;
  /** A stream live this long starts the backoff over. */
  resetAfterMs?: number;
  now?: () => number;
  random?: () => number;
}

/** Errors that a later attempt cannot fix. */
const FINAL = new Set<ErrorCode>([
  ErrorCode.UNIMPLEMENTED,
  ErrorCode.INVALID_ARGUMENT,
  ErrorCode.NOT_FOUND,
  ErrorCode.UNAUTHENTICATED,
]);

/** Subscribes to `method`, again and again, until stopped. */
export function subscribe<M extends StreamMethod>(
  client: StreamClient,
  method: M,
  request: RequestFor<M>,
  handlers: {
    onItem(item: ItemFor<M>, generation: number): void;
    onStatus(status: LiveStatus): void;
  },
  options: RetryOptions = {},
): Subscription {
  const min = options.backoffMinMs ?? 500;
  const max = options.backoffMaxMs ?? 30_000;
  const resetAfter = options.resetAfterMs ?? 10_000;
  const now = options.now ?? Date.now;
  const random = options.random ?? Math.random;
  let stopped = false;
  let controller: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  let attempt = 0;
  // Statuses go out on a microtask, never inside the caller's own work.
  const status = (value: LiveStatus) =>
    queueMicrotask(() => {
      if (!stopped) handlers.onStatus(value);
    });

  const retry = (message: string) => {
    attempt++;
    status({ kind: "retrying", message, attempt });
    const delay = Math.min(max, min * 2 ** (attempt - 1)) * (0.5 + random());
    timer = setTimeout(start, delay);
  };

  function start(): void {
    if (stopped) return;
    timer = undefined;
    generation++;
    const mine = generation;
    controller = new AbortController();
    let liveSince: number | undefined;
    status({ kind: "connecting" });
    client.stream(method, request, (item) => {
      if (stopped || mine !== generation) return;
      if (liveSince === undefined) {
        liveSince = now();
        status({ kind: "live" });
      }
      handlers.onItem(item, mine);
    }, { signal: controller.signal }).then(
      () => {
        if (stopped || mine !== generation) return;
        if (liveSince !== undefined && now() - liveSince >= resetAfter) {
          attempt = 0;
        }
        retry("the stream ended");
      },
      (error: unknown) => {
        if (stopped || mine !== generation) return;
        const rpc = error instanceof RpcError ? error : undefined;
        if (rpc && FINAL.has(rpc.code)) {
          status({ kind: "failed", message: rpc.message, code: rpc.code });
          return;
        }
        if (liveSince !== undefined && now() - liveSince >= resetAfter) {
          attempt = 0;
        }
        retry(error instanceof Error ? error.message : String(error));
      },
    );
  }

  start();
  return {
    stop() {
      stopped = true;
      clearTimeout(timer);
      controller?.abort();
    },
  };
}
