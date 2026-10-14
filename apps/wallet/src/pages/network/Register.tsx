/**
 * Registration (spec 6 §1.6.11): this node's standing with each bootstrap
 * node, and registering again now, with feedback (UI-4).
 */

import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { relativeTime, shortId } from "../../shared/format.ts";
import { createLiveValue } from "../../shared/live/live.ts";
import { describe, useClient } from "../../shared/rpc.ts";
import { useToast } from "../../shared/ui/toast.tsx";
import {
  Badge,
  Card,
  ErrorText,
  LiveNote,
  PageTitle,
} from "../../shared/ui/ui.tsx";

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
    <div class="page-narrow">
      <PageTitle>Register</PageTitle>
      <LiveNote status={status.status()} />
      <Card testid="page-register">
        <p class="muted">
          Your node registers with the network's bootstrap nodes by itself, and
          renews before a registration lapses. You can also renew now.
        </p>
        <ul class="bootstrap-list">
          <For each={status.value()?.bootstrap ?? []}>
            {(entry) => (
              <li data-testid={`bootstrap-${entry.node}`}>
                <code>{shortId(entry.node, 20)}</code>{" "}
                <Show
                  when={entry.listed}
                  fallback={<Badge tone="bad">Not registered</Badge>}
                >
                  <Badge tone="good">Registered</Badge>
                </Show>
                <div class="muted">
                  registered{" "}
                  {relativeTime(entry.registeredAtUnixMs, Date.now())}, list
                  fetched {relativeTime(entry.listFetchedUnixMs, Date.now())}
                </div>
                <button
                  type="button"
                  data-testid={`renew-${entry.node}`}
                  disabled={busy() !== undefined}
                  onClick={() => renew(entry.node)}
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
