/**
 * Registration (spec/areas/wallet.md#pages): this node's standing with each
 * bootstrap node, and registering again now, with feedback.
 */

import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { relativeTime, shortId } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import { Badge, Card, ErrorText, LiveNote, PageTitle } from "@dassie/ui/ui";

export function Register(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const status = createLiveValue(
    "subscribeRegistration",
    () => ({}),
    (item) => item,
  );
  const [busy, setBusy] = createSignal<string>();
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const renew = async (node: string) => {
    setBusy(node);
    setErrors((all) => ({ ...all, [node]: "" }));
    try {
      await client.call("registerWithBootstrap", { node });
      toast({ title: "Registration sent" });
    } catch (failure) {
      setErrors((all) => ({ ...all, [node]: describe(failure) }));
    }
    setBusy(undefined);
  };
  return (
    <div class="page-narrow stack">
      <PageTitle>Register</PageTitle>
      <LiveNote status={status.status()} />
      <Card testid="page-register">
        <p class="muted">
          Your node registers with the network's bootstrap nodes by itself, and
          renews before a registration lapses. You can also renew now.
        </p>
        <ul class="node-rows">
          <For each={status.value()?.bootstrap ?? []}>
            {(entry) => (
              <li class="node-row" data-testid={`bootstrap-${entry.node}`}>
                <code class="node-name">{shortId(entry.node, 20)}</code>{" "}
                <Show
                  when={entry.listed}
                  fallback={<Badge tone="bad">Not registered</Badge>}
                >
                  <Badge tone="good">Registered</Badge>
                </Show>
                <span class="node-status">
                  registered{" "}
                  {relativeTime(entry.registeredAtUnixMs, Date.now())}, list
                  fetched {relativeTime(entry.listFetchedUnixMs, Date.now())}
                </span>
                <button
                  type="button"
                  data-testid={`renew-${entry.node}`}
                  disabled={busy() !== undefined}
                  onClick={() =>
                    renew(entry.node)}
                >
                  Renew now
                </button>
                <ErrorText message={errors()[entry.node]} />
              </li>
            )}
          </For>
        </ul>
        <Show when={(status.value()?.bootstrap.length ?? 0) === 0}>
          <p class="muted">
            This node has no bootstrap nodes: it runs alone until{" "}
            <code>DASSIE_BOOTSTRAP_NODES</code> names some.
          </p>
        </Show>
      </Card>
    </div>
  );
}
