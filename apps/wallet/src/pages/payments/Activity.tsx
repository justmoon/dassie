/**
 * Activity: the incoming payments, newest first, as they receive money
 * (D9). Outgoing payments join them with Send (M6c).
 */

import type { JSX } from "@solidjs/web";
import type { IncomingPayment } from "@dassie/rpc/proto";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { dateTime } from "@dassie/ui/format";
import { createLiveTable } from "@dassie/ui/live";
import { describe, useClient } from "@dassie/ui/rpc";
import {
  Badge,
  Card,
  ConfirmButton,
  ErrorText,
  LiveNote,
  PageTitle,
} from "@dassie/ui/ui";
import {
  clientLabel,
  description,
  incomingState,
  newestFirst,
} from "./incoming.ts";

/** How many incoming payments the page follows. */
const LIMIT = 100;

export function Activity(): JSX.Element {
  const live = createLiveTable(
    "subscribeIncomingPayments",
    () => ({ limit: LIMIT }),
    (item) => item,
    (payment: IncomingPayment) => payment.id,
  );
  const payments = () => newestFirst(Object.values(live.state.byId));
  // Expiry is a matter of time: the page looks again every few seconds.
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 5_000);
  onCleanup(() => clearInterval(timer));
  return (
    <div class="page-wide stack">
      <PageTitle>Activity</PageTitle>
      <Card title="Incoming Payments" testid="page-activity">
        <LiveNote status={live.status()} />
        <Show
          when={payments().length > 0}
          fallback={
            <p class="muted" data-testid="activity-empty">
              No incoming payments yet. Request one under Receive.
            </p>
          }
        >
          <ul class="activity">
            <For each={payments()} keyed={(payment) => payment.id}>
              {(payment) => <Row payment={payment()} now={now()} />}
            </For>
          </ul>
        </Show>
      </Card>
    </div>
  );
}

function Row(props: { payment: IncomingPayment; now: number }): JSX.Element {
  const client = useClient();
  const [error, setError] = createSignal<string>();
  const state = () => incomingState(props.payment, props.now);
  const currency = () => ledgerCurrency(props.payment);
  const complete = async () => {
    setError(undefined);
    try {
      await client.call("completeIncomingPayment", { id: props.payment.id });
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <li data-testid={`incoming-${props.payment.id}`} data-state={state()}>
      <div class="activity-head">
        <strong>
          {description(props.payment.metadataJson) ?? "Incoming payment"}
        </strong>
        <Badge
          tone={state() === "completed"
            ? "good"
            : state() === "expired"
            ? "bad"
            : "plain"}
          testid="incoming-state"
        >
          {state()}
        </Badge>
      </div>
      <p>
        <Amount
          value={props.payment.receivedAmount}
          currency={currency()}
          testid="incoming-received"
        />
        <Show when={props.payment.incomingAmount !== undefined}>
          {" of "}
          <Amount
            value={props.payment.incomingAmount ?? 0n}
            currency={currency()}
          />
        </Show>
      </p>
      <p class="muted">
        Requested by {clientLabel(props.payment.client)},{" "}
        {dateTime(props.payment.createdAtMs)}
      </p>
      <Show when={state() === "open"}>
        <ConfirmButton
          label="Complete"
          title="Complete this payment?"
          confirm="Complete"
          onConfirm={complete}
          testid="incoming-complete"
        >
          <p>It takes no more money, whatever it was asked for.</p>
        </ConfirmButton>
      </Show>
      <ErrorText message={error()} />
    </li>
  );
}
