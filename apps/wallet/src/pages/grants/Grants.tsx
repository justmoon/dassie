/**
 * Connected apps (spec/decisions/open-payments.md): the grants the owner
 * approved, what each may do and what it spent this period, and a way to
 * revoke each.
 */

import type { JSX } from "@solidjs/web";
import type { Grant } from "@dassie/rpc/proto";
import { createSignal, For, Show } from "solid-js";
import { dateTime } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { describe, useClient } from "@dassie/ui/rpc";
import {
  Card,
  ConfirmButton,
  ErrorText,
  LiveNote,
  PageTitle,
} from "@dassie/ui/ui";
import { accessText, clientName, spentText } from "./grants.ts";

export function Grants(): JSX.Element {
  const live = createLiveValue(
    "subscribeGrants",
    () => ({}),
    (item) => item.grants,
  );
  return (
    <div class="page-wide stack">
      <PageTitle>Connected Apps</PageTitle>
      <Card testid="page-grants">
        <LiveNote status={live.status()} />
        <Show
          when={(live.value()?.length ?? 0) > 0}
          fallback={
            <p class="muted" data-testid="grants-empty">
              No app has access. When an app asks, you approve it here first.
            </p>
          }
        >
          <ul class="grants">
            <For each={live.value() ?? []} keyed={(grant) => grant.id}>
              {(grant) => <Row grant={grant()} />}
            </For>
          </ul>
        </Show>
      </Card>
    </div>
  );
}

function Row(props: { grant: Grant }): JSX.Element {
  const client = useClient();
  const [error, setError] = createSignal<string>();
  const revoke = async () => {
    setError(undefined);
    try {
      await client.call("revokeGrant", { id: props.grant.id });
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <li data-testid={`grant-${props.grant.id}`}>
      <div class="grant-head">
        <strong>
          {clientName(props.grant.client, props.grant.clientName)}
        </strong>
        <span class="muted">
          since{" "}
          {dateTime(props.grant.decidedAtUnixMs ?? props.grant.createdAtUnixMs)}
        </span>
      </div>
      <ul class="grant-access">
        <For each={props.grant.access}>
          {(access) => (
            <li>
              {accessText(access)}
              <Show when={access.limits && spentText(access.limits)}>
                {(spent) => (
                  <span class="muted" data-testid="grant-spent">
                    — {spent()}
                  </span>
                )}
              </Show>
            </li>
          )}
        </For>
      </ul>
      <ConfirmButton
        label="Revoke"
        title="Revoke this app's access?"
        confirm="Revoke"
        tone="danger"
        onConfirm={revoke}
        testid="grant-revoke"
      >
        <p>
          It can do nothing more for you. Payments it already made stay made.
        </p>
      </ConfirmButton>
      <ErrorText message={error()} />
    </li>
  );
}
