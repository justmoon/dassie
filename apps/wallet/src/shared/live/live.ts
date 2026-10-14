/**
 * Live views for components: a stream that stays open while its component
 * is shown, feeding a signal or a store (spec 7 §5). Leaving the page ends
 * the stream; a stream that fails is subscribed again (`subscribe`).
 */

import type { ItemFor, RequestFor, RpcClient, StreamMethod } from "@dassie/rpc";
import {
  type Accessor,
  createEffect,
  createSignal,
  createStore,
  type Store,
} from "solid-js";
import { useClient } from "../rpc.ts";
import { applyTableEvent, emptyKeyed, type Keyed, pushRing } from "./reduce.ts";
import { type LiveStatus, subscribe } from "./subscription.ts";

/** Keeps `method` subscribed with `request` while the owner lives. */
function follow<M extends StreamMethod>(
  client: RpcClient,
  method: M,
  request: () => RequestFor<M>,
  onItem: (item: ItemFor<M>, fresh: boolean) => void,
  setStatus: (status: LiveStatus) => void,
): void {
  createEffect(() => ({ current: request() }), ({ current }) => {
    let generation = 0;
    const subscription = subscribe(client, method, current, {
      onItem: (item, mine) => {
        onItem(item, mine !== generation);
        generation = mine;
      },
      onStatus: setStatus,
    });
    return () => subscription.stop();
  });
}

/** A value the node streams whole, as it changes. */
export function createLiveValue<M extends StreamMethod, T>(
  method: M,
  request: () => RequestFor<M>,
  select: (item: ItemFor<M>, fresh: boolean) => T,
  client: RpcClient = useClient(),
): { value: Accessor<T | undefined>; status: Accessor<LiveStatus> } {
  const [value, setValue] = createSignal<T | undefined>(undefined, {
    equals: false,
  });
  const [status, setStatus] = createSignal<LiveStatus>({ kind: "connecting" });
  follow(
    client,
    method,
    request,
    (item, fresh) => setValue(() => select(item, fresh)),
    setStatus,
  );
  return { value, status };
}

/** A table the node streams as a snapshot and changes, by key. */
export function createLiveTable<M extends StreamMethod, T>(
  method: M,
  request: () => RequestFor<M>,
  event: (item: ItemFor<M>) => {
    reset: boolean;
    upserted: readonly T[];
    removed: readonly string[];
  },
  key: (entry: T) => string,
  client: RpcClient = useClient(),
): { state: Store<Keyed<T>>; status: Accessor<LiveStatus> } {
  const [state, setState] = createStore<Keyed<T>>(emptyKeyed<T>());
  const [status, setStatus] = createSignal<LiveStatus>({ kind: "connecting" });
  follow(client, method, request, (item) => {
    setState((draft) => {
      applyTableEvent(draft as Keyed<T>, event(item), key);
    });
  }, setStatus);
  return { state, status };
}

/**
 * The newest lines of a stream, at most `max` of them: a plain array read
 * through a version, since thousands of lines need no store.
 */
export function createLiveBuffer<M extends StreamMethod, T>(
  method: M,
  request: () => RequestFor<M>,
  lines: (item: ItemFor<M>) => { reset: boolean; lines: readonly T[] },
  max: number,
  client: RpcClient = useClient(),
): { lines: Accessor<readonly T[]>; status: Accessor<LiveStatus> } {
  const ring: T[] = [];
  const [version, setVersion] = createSignal(0);
  const [status, setStatus] = createSignal<LiveStatus>({ kind: "connecting" });
  follow(client, method, request, (item, fresh) => {
    const event = lines(item);
    if (event.reset || fresh) ring.length = 0;
    pushRing(ring, event.lines, max);
    setVersion((n) => n + 1);
  }, setStatus);
  return {
    lines: () => {
      version();
      return ring;
    },
    status,
  };
}
