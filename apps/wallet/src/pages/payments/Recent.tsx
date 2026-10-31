/**
 * Home's last few payments, in and out, from the streams Activity follows,
 * each asked for its newest few; Activity has them all. A row is Kopje's:
 * a pebble with the value cube for money in or the send glyph for money
 * out, whom it was with and when, and the amount, money in in green.
 */

import type { JSX } from "@solidjs/web";
import type { IncomingPayment, OutgoingPayment } from "@dassie/rpc/proto";
import { createSignal, createUniqueId, For, onCleanup, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { relativeTime } from "@dassie/ui/format";
import { createLiveTable } from "@dassie/ui/live";
import { Link } from "@dassie/ui/router";
import { LiveNote, ValueCube } from "@dassie/ui/ui";
import { description, incomingState, shownReceived } from "./incoming.ts";
import { recent, type RecentPayment } from "./recent.ts";
import { destinationLabel, stateText } from "./send.ts";
import { SendIcon } from "./icons.tsx";

/** How many payments home lists. */
const COUNT = 5;

export function Recent(): JSX.Element {
  const id = createUniqueId();
  const incoming = createLiveTable(
    "subscribeIncomingPayments",
    () => ({ limit: COUNT }),
    (item) => item,
    (payment: IncomingPayment) => payment.id,
  );
  const outgoing = createLiveTable(
    "subscribeOutgoingPayments",
    () => ({ limit: COUNT }),
    (item) => item,
    (payment: OutgoingPayment) => payment.id,
  );
  // One note for both: the first that is not live.
  const status = () =>
    incoming.status().kind === "live" ? outgoing.status() : incoming.status();
  const payments = () =>
    recent(
      Object.values(incoming.state.byId),
      Object.values(outgoing.state.byId),
      COUNT,
    );
  // How long ago is a matter of time: the list looks again now and then.
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 15_000);
  onCleanup(() => clearInterval(timer));
  return (
    <section class="home-recent" aria-labelledby={id} data-testid="recent">
      <div class="section-head">
        <h2 class="overline" id={id}>Recent</h2>
        <Link href="/payments" testid="recent-activity">All activity</Link>
      </div>
      <LiveNote status={status()} />
      <Show
        when={payments().length > 0}
        fallback={
          <p class="muted" data-testid="recent-empty">No payments yet.</p>
        }
      >
        <ul class="tx-list">
          <For each={payments()} keyed={(item) => item.key}>
            {(item) => (
              <>
                <Show when={paymentIn(item())}>
                  {(payment) => <IncomingRow payment={payment()} now={now()} />}
                </Show>
                <Show when={paymentOut(item())}>
                  {(payment) => <OutgoingRow payment={payment()} now={now()} />}
                </Show>
              </>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}

type Entry = RecentPayment<IncomingPayment, OutgoingPayment>;

const paymentIn = (entry: Entry) =>
  entry.direction === "in" ? entry.payment : undefined;

const paymentOut = (entry: Entry) =>
  entry.direction === "out" ? entry.payment : undefined;

function IncomingRow(
  props: { payment: IncomingPayment; now: number },
): JSX.Element {
  return (
    <li
      class="tx"
      data-testid={`recent-in-${props.payment.id}`}
      data-state={incomingState(props.payment, props.now)}
    >
      <TxAvatar direction="in" />
      <div class="tx-main">
        <p class="tx-name">
          {description(props.payment.metadataJson) ?? "Incoming payment"}
        </p>
        <p class="tx-detail">
          {incomingState(props.payment, props.now)} ·{" "}
          {relativeTime(props.payment.createdAtUnixMs, props.now)}
        </p>
      </div>
      <p class="tx-amount tx-in">
        {/* A word joiner keeps the sign with its amount when it wraps. */}
        {"+\u2060"}<Amount
          value={shownReceived(props.payment)}
          currency={ledgerCurrency(props.payment)}
        />
      </p>
    </li>
  );
}

function OutgoingRow(
  props: { payment: OutgoingPayment; now: number },
): JSX.Element {
  return (
    <li
      class="tx"
      data-testid={`recent-out-${props.payment.id}`}
      data-state={props.payment.state}
    >
      <TxAvatar direction="out" />
      <div class="tx-main">
        <p class="tx-name">
          <Link href={`/payments/${props.payment.id}`}>
            To {destinationLabel(props.payment)}
          </Link>
        </p>
        <p class="tx-detail">
          {stateText(props.payment.state)} ·{" "}
          {relativeTime(props.payment.createdAtUnixMs, props.now)}
        </p>
      </div>
      <p class="tx-amount">
        {"−\u2060"}<Amount
          value={props.payment.sentAmount}
          currency={ledgerCurrency(props.payment)}
        />
      </p>
    </li>
  );
}

/** A row's pebble: the value cube for money in, the send glyph for money
 * out. */
export function TxAvatar(props: { direction: "in" | "out" }): JSX.Element {
  return (
    <span class={`tx-av tx-av-${props.direction}`} aria-hidden="true">
      <Show when={props.direction === "in"} fallback={<SendIcon />}>
        <ValueCube />
      </Show>
    </span>
  );
}
