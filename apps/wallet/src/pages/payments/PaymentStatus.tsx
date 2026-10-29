/**
 * One outgoing payment, live (spec/decisions/open-payments.md). It leads
 * with the amount the payment fixed: what the receiver gets, or what it
 * sends of what it debits. The other amount follows. The owner's Send
 * leads here; so does each outgoing payment in Activity.
 */

import type { JSX } from "@solidjs/web";
import type { OutgoingPayment } from "@dassie/rpc/proto";
import { Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { dateTime } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { Link, type RouteProps } from "@dassie/ui/router";
import { Badge, Card, LiveNote, PageTitle } from "@dassie/ui/ui";
import { clientLabel } from "./incoming.ts";
import { destinationLabel, paymentProgress, stateText } from "./send.ts";

export function PaymentStatus(props: RouteProps): JSX.Element {
  const live = createLiveValue(
    "subscribeOutgoingPayment",
    () => ({ id: props.params.id ?? "" }),
    (item) => item,
  );
  return (
    <div class="page-narrow stack" data-testid="page-payment">
      <PageTitle>Payment</PageTitle>
      <LiveNote status={live.status()} />
      <Show when={live.value()}>
        {(payment) => <Status payment={payment()} />}
      </Show>
      <p>
        <Link href="/payments" class="button" testid="payment-activity">
          All activity
        </Link>
      </p>
    </div>
  );
}

function Status(props: { payment: OutgoingPayment }): JSX.Element {
  const currency = () => ledgerCurrency(props.payment);
  const tone = () =>
    props.payment.state === "completed"
      ? "good"
      : props.payment.state === "failed"
      ? "bad"
      : "plain";
  const completed = () => props.payment.state === "completed";
  const receive = () => props.payment.receiveAmount;
  // What it sent so far, and of the most it may take until it is done.
  const sent = (size?: "lg") => (
    <>
      <Amount
        value={props.payment.sentAmount}
        currency={currency()}
        size={size}
        testid="payment-sent"
      />
      <Show when={!completed()}>
        {" of "}
        <Amount value={props.payment.debitAmount} currency={currency()} />
      </Show>
    </>
  );
  // What the receiver said it received so far, and of what it is to get
  // until it is done; then what it was to get, as it counts as paid in
  // full.
  const received = (size?: "lg") => (
    <Show when={receive()}>
      {(amount) => (
        <Show
          when={!completed()}
          fallback={
            <Amount
              value={amount().value}
              currency={ledgerCurrency(amount())}
              size={size}
              testid="payment-receive"
            />
          }
        >
          <Amount
            value={props.payment.deliveredAmount}
            currency={ledgerCurrency(amount())}
            size={size}
            testid="payment-delivered"
          />
          {" of "}
          <Amount
            value={amount().value}
            currency={ledgerCurrency(amount())}
            testid="payment-receive"
          />
        </Show>
      )}
    </Show>
  );
  return (
    <Card title={`To ${destinationLabel(props.payment)}`}>
      <div
        class="stack"
        data-testid="payment"
        data-state={props.payment.state}
      >
        <p>
          <Badge tone={tone()} testid="payment-state">
            {stateText(props.payment.state)}
          </Badge>
        </p>
        <p class="payment-lead">
          <Show when={props.payment.fixedDebit} fallback={received("lg")}>
            {sent("lg")}
          </Show>
        </p>
        <progress
          class="payment-progress"
          max="1000"
          value={paymentProgress(props.payment)}
          aria-label="Sent so far"
        />
        <Show when={props.payment.failure !== ""}>
          <p class="error" role="alert" data-testid="payment-failure">
            {props.payment.failure}
          </p>
        </Show>
        <dl class="facts rows">
          <Show
            when={props.payment.fixedDebit}
            fallback={
              <>
                <dt>
                  {props.payment.state === "sending"
                    ? "Sent so far"
                    : "You paid"}
                </dt>
                <dd>{sent()}</dd>
              </>
            }
          >
            <dt>They receive</dt>
            <dd>{received()}</dd>
          </Show>
          <dt>Recipient</dt>
          <dd data-testid="payment-recipient">
            {destinationLabel(props.payment)}
          </dd>
          <dt>Sent by</dt>
          <dd>{clientLabel(props.payment.client)}</dd>
          <dt>Started</dt>
          <dd>{dateTime(props.payment.createdAtUnixMs)}</dd>
          <Show when={props.payment.finishedAtUnixMs}>
            {(at) => (
              <>
                <dt>Finished</dt>
                <dd>{dateTime(at())}</dd>
              </>
            )}
          </Show>
        </dl>
        <Show when={props.payment.recipientUrl !== ""}>
          <div class="actions">
            <Link
              href={`/send?to=${
                encodeURIComponent(props.payment.recipientUrl)
              }`}
              class="button"
              testid="payment-send-again"
            >
              Send again
            </Link>
          </div>
        </Show>
      </div>
    </Card>
  );
}
