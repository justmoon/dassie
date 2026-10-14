/**
 * Asking for a payment: an incoming payment the owner creates, with an
 * amount or without, whose URL another wallet pays, and whose STREAM
 * credentials a STREAM sender pays directly.
 */

import type { JSX } from "@solidjs/web";
import type { IncomingPayment } from "@dassie/rpc/proto";
import { createSignal, createUniqueId, For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { parseAmount, parseFailureText } from "@dassie/ui/currency";
import { hex } from "@dassie/ui/format";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { describe, useClient } from "@dassie/ui/rpc";
import { CopyButton, ErrorText, Field, Masked } from "@dassie/ui/ui";

export function RequestPayment(): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const id = createUniqueId();
  const [ledger, setLedger] = createSignal<string>();
  const [amount, setAmount] = createSignal("");
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  const [created, setCreated] = createSignal<IncomingPayment>();
  const chosen = () =>
    data.ledgers()?.find((entry) => entry.id === ledger()) ??
      data.ledgers()?.[0];
  const create = async (event: SubmitEvent) => {
    event.preventDefault();
    if (pending()) return;
    setError(undefined);
    const scale = chosen()?.assetScale ?? 9;
    let incomingAmount: bigint | undefined;
    if (amount().trim() !== "") {
      const parsed = parseAmount(amount(), scale);
      if (!parsed.ok) return setError(parseFailureText(parsed.reason, scale));
      if (parsed.value === 0n) return setError("Ask for more than nothing.");
      incomingAmount = parsed.value;
    }
    setPending(true);
    try {
      const response = await client.call("createIncomingPayment", {
        ledger: ledger() ?? "",
        incomingAmount,
        description: text(),
      });
      setCreated(response.payment);
      setAmount("");
      setText("");
    } catch (failure) {
      setError(describe(failure));
    }
    setPending(false);
  };
  return (
    <div class="stack">
      <form class="request-payment" onSubmit={create}>
        <h3>Request a Payment</h3>
        <Show when={(data.ledgers()?.length ?? 0) > 1}>
          <Field label="Ledger" for={`${id}-ledger`}>
            <select
              id={`${id}-ledger`}
              data-testid="request-ledger"
              onChange={(event) => setLedger(event.currentTarget.value)}
            >
              <For each={data.ledgers() ?? []}>
                {(entry) => (
                  <option value={entry.id} selected={entry.id === chosen()?.id}>
                    {entry.id}
                  </option>
                )}
              </For>
            </select>
          </Field>
        </Show>
        <Field
          label={`Amount (${chosen()?.assetCode ?? ""})`}
          for={`${id}-amount`}
          hint="Leave empty to take any amount."
        >
          <input
            id={`${id}-amount`}
            inputmode="decimal"
            data-testid="request-amount"
            placeholder="any amount"
            value={amount()}
            onInput={(event) => setAmount(event.currentTarget.value)}
          />
        </Field>
        <Field label="What it is for" for={`${id}-description`}>
          <input
            id={`${id}-description`}
            data-testid="request-description"
            maxlength="256"
            value={text()}
            onInput={(event) => setText(event.currentTarget.value)}
          />
        </Field>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-testid="request-create"
            disabled={pending()}
          >
            Create Request
          </button>
        </div>
        <ErrorText message={error()} />
      </form>
      <Show when={created()}>
        {(payment) => <Created payment={payment()} />}
      </Show>
    </div>
  );
}

/** The new request: its URL to share, and its STREAM credentials. */
function Created(props: { payment: IncomingPayment }): JSX.Element {
  const currency = () => ledgerCurrency(props.payment);
  return (
    <div class="stack" data-testid="request-created">
      <p>
        Share this link with whoever pays you
        <Show when={props.payment.incomingAmount !== undefined}>
          {" "}
          <Amount
            value={props.payment.incomingAmount ?? 0n}
            currency={currency()}
          />
        </Show>
        :
      </p>
      <p class="boxed">
        <code data-testid="request-url">{props.payment.url}</code>{" "}
        <CopyButton text={props.payment.url} />
      </p>
      <details>
        <summary data-testid="stream-credentials">STREAM credentials</summary>
        <dl class="facts">
          <dt>Destination</dt>
          <dd>
            <code data-testid="stream-destination">
              {props.payment.destinationAccount}
            </code>
            <CopyButton text={props.payment.destinationAccount} />
          </dd>
          <dt>Shared secret</dt>
          <dd>
            <Masked
              text={hex(props.payment.sharedSecret)}
              testid="stream-secret"
            />
            <CopyButton text={hex(props.payment.sharedSecret)} />
          </dd>
        </dl>
      </details>
      <p>
        <Link href="/payments" testid="request-activity">
          See it in your activity
        </Link>
      </p>
    </div>
  );
}
