/**
 * The line naming the node's rate sources, wherever the wallet shows a rate
 * or a total (spec/areas/wallet.md#pages,
 * spec/decisions/open-rate-sources.md): the central banks' terms let their
 * rates be used if they are named as the source.
 */

import type { JSX } from "@solidjs/web";
import { createMemo, Show } from "solid-js";
import { useClient } from "@dassie/ui/rpc";
import { sourcesText } from "./rates.ts";

export function RateSources(props: { class?: string }): JSX.Element {
  const client = useClient();
  const text = createMemo(async () => {
    try {
      return sourcesText((await client.call("getRateSources", {})).sources);
    } catch {
      return "";
    }
  });
  return (
    <Show when={text()}>
      {(line) => (
        <p class={props.class ?? "muted"} data-testid="rate-sources-line">
          {line()}
        </p>
      )}
    </Show>
  );
}
