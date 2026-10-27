/**
 * Activity: the incoming and outgoing payments, newest first, as they
 * receive and send money (spec/decisions/open-payments.md). Outgoing payments
 * are the owner's Sends and those of apps the owner approved; each leads to
 * its own page.
 */

import type { JSX } from "@solidjs/web";
import type { IncomingPayment, OutgoingPayment } from "@dassie/rpc/proto";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { dateTime } from "@dassie/ui/format";
import { createLiveTable } from "@dassie/ui/live";
import { Link } from "@dassie/ui/router";
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
import { destinationLabel } from "./send.ts";

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
      <OutgoingPayments />
    </div>
  );
}

/** The outgoing payments, live. */
function OutgoingPayments(): JSX.Element {
  const live = createLiveTable(
    "subscribeOutgoingPayments",
    () => ({ limit: LIMIT }),
    (item) => item,
    (payment: OutgoingPayment) => payment.id,
  );
  const payments = () =>
    [...Object.values(live.state.byId)].sort((a, b) =>
      a.createdAtUnixMs === b.createdAtUnixMs
        ? a.id.localeCompare(b.id)
        : a.createdAtUnixMs > b.createdAtUnixMs
        ? -1
        : 1
    );
  return (
    <Card title="Outgoing Payments" testid="outgoing-payments">
      <LiveNote status={live.status()} />
      <Show
        when={payments().length > 0}
        fallback={
          <p class="muted" data-testid="outgoing-empty">
            No outgoing payments yet.
          </p>
        }
      >
        <ul class="activity">
          <For each={payments()} keyed={(payment) => payment.id}>
            {(payment) => <OutgoingRow payment={payment()} />}
          </For>
        </ul>
      </Show>
    </Card>
  );
}

function OutgoingRow(props: { payment: OutgoingPayment }): JSX.Element {
  const currency = () => ledgerCurrency(props.payment);
  const tone = () =>
    props.payment.state === "completed"
      ? "good"
      : props.payment.state === "failed"
      ? "bad"
      : "plain";
  return (
    <li
      data-testid={`outgoing-${props.payment.id}`}
      data-state={props.payment.state}
    >
      <div class="activity-head">
        <Link
          href={`/payments/${props.payment.id}`}
          testid="outgoing-link"
        >
          <strong>To {destinationLabel(props.payment)}</strong>
        </Link>
        <Badge tone={tone()} testid="outgoing-state">
          {props.payment.state}
        </Badge>
      </div>
      <p>
        <Amount
          value={props.payment.sentAmount}
          currency={currency()}
          testid="outgoing-sent"
        />
        {" of "}
        <Amount value={props.payment.debitAmount} currency={currency()} />
      </p>
      <p class="muted">
        Sent by {clientLabel(props.payment.client)},{" "}
        {dateTime(props.payment.createdAtUnixMs)}
      </p>
      <Show when={props.payment.failure !== ""}>
        <p class="muted" data-testid="outgoing-failure">
          {props.payment.failure}
        </p>
      </Show>
    </li>
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
        {dateTime(props.payment.createdAtUnixMs)}
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
