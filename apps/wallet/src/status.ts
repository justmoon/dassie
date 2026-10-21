/**
 * The node's status (`GetStatus`), asked again every little while, for
 * what it says that no live stream carries: whether the node is a
 * periphery node, and how it stands with its relay.
 */

import type { GetStatusResponse } from "@dassie/rpc/proto";
import { type Accessor, createSignal, onCleanup } from "solid-js";
import { useClient } from "@dassie/ui/rpc";

/** How often the status is asked again. */
const STATUS_POLL_MS = 15_000;

/** The status, `undefined` until known; a failed poll keeps what is known. */
export function useStatus(): Accessor<GetStatusResponse | undefined> {
  const client = useClient();
  const [status, setStatus] = createSignal<GetStatusResponse>();
  const ask = async () => {
    try {
      setStatus(await client.call("getStatus", {}));
    } catch {
      // Keep what is known; the next poll asks again.
    }
  };
  void ask();
  const timer = setInterval(() => void ask(), STATUS_POLL_MS);
  onCleanup(() => clearInterval(timer));
  return status;
}
