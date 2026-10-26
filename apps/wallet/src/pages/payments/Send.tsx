/**
 * Send (spec/decisions/open-payments.md, M6c, M7c): to a payment pointer, a
 * wallet address, or a payment request, on this node or another, in any
 * currency. The owner fixes what the recipient gets or what they send; the
 * node reads the recipient, measures the rate of the route, quotes what the
 * payment debits and delivers, and pays once the owner confirms; the
 * payment's own page follows it from there.
 */

import type { JSX } from "@solidjs/web";
import type { Recipient, SendQuote } from "@dassie/rpc/proto";
import { createSignal, createUniqueId, For, onCleanup, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { amountText, parseAmount, parseFailureText } from "@dassie/ui/currency";
import { useNodeData } from "@dassie/ui/node-data";
import { useRouter, useSearchParam } from "@dassie/ui/router";
import { describe, useClient } from "@dassie/ui/rpc";
import { Card, ErrorText, Field, PageTitle } from "@dassie/ui/ui";
import {
  type Fixed,
  lapseText,
  looksLikeRecipient,
  quoteAmount,
  recipientName,
  type Step,
} from "./send.ts";
import { useWalletAddresses } from "./WalletAddresses.tsx";

/** How long typing pauses before the recipient is looked up. */
const LOOKUP_PAUSE_MS = 400;

export function Send(): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const router = useRouter();
  const id = createUniqueId();
  const to = useSearchParam("to");
  const addresses = useWalletAddresses();
  const [step, setStep] = createSignal<Step>("recipient");
  const [input, setInput] = createSignal(to() ?? "");
  const [recipient, setRecipient] = createSignal<Recipient>();
  const [ledger, setLedger] = createSignal<string>();
  const [amount, setAmount] = createSignal("");
  const [fixed, setFixed] = createSignal<Fixed>("receive");
  const [note, setNote] = createSignal("");
  const [quote, setQuote] = createSignal<SendQuote>();
  const [error, setError] = createSignal<string>();
  const [pending, setPending] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const ticker = setInterval(() => setNow(Date.now()), 1_000);
  onCleanup(() => clearInterval(ticker));

  // The ledger that pays: the one chosen, else one in the recipient's
  // asset, else the default, `$host`'s. The node pays from the ledger the
  // route starts on, and says so if it is another.
  const ledgers = () => data.ledgers() ?? [];
  const chosen = () => {
    const all = ledgers();
    const same = all.find((entry) =>
      entry.assetCode === recipient()?.assetCode &&
      entry.assetScale === recipient()?.assetScale
    );
    const wanted = ledger() ?? same?.id ??
      addresses.addresses()?.find((address) => address.isDefault)?.ledger;
    return all.find((entry) => entry.id === wanted) ?? all[0];
  };
  // The amount is in the recipient's asset, or the paying ledger's.
  const scale = () =>
    fixed() === "send"
      ? chosen()?.assetScale ?? 9
      : recipient()?.assetScale ?? 9;
  const amountLabel = () =>
    fixed() === "send"
      ? `Amount you send (${chosen()?.assetCode ?? "?"})`
      : `Amount they receive (${recipient()?.assetCode ?? "?"})`;

  // Looks the recipient up: after a pause in typing, or on Continue. Only
  // the latest lookup counts.
  let asked = 0;
  let pause: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(pause));
  const lookUp = async (text: string): Promise<Recipient | undefined> => {
    const mine = ++asked;
    setPending(true);
    setError(undefined);
    try {
      const response = await client.call("resolveRecipient", {
        recipient: text,
      });
      if (mine !== asked) return undefined;
      setRecipient(response.recipient);
      return response.recipient;
    } catch (failure) {
      if (mine === asked) {
        setRecipient(undefined);
        setError(describe(failure));
      }
      return undefined;
    } finally {
      if (mine === asked) setPending(false);
    }
  };
  const typed = (text: string) => {
    setInput(text);
    setRecipient(undefined);
    setError(undefined);
    clearTimeout(pause);
    // A lookup still on its way is for what was there before.
    asked++;
    setPending(false);
    if (looksLikeRecipient(text)) {
      pause = setTimeout(() => void lookUp(text), LOOKUP_PAUSE_MS);
    }
  };
  if (to()) void lookUp(to() ?? "");

  const toAmount = async (event: SubmitEvent) => {
    event.preventDefault();
    if (pending()) return;
    clearTimeout(pause);
    const found = recipient() ?? await lookUp(input());
    if (!found) return;
    // A request that asks for an amount pays that, unless changed.
    const remaining = found.request?.remaining;
    setFixed("receive");
    setAmount(
      remaining !== undefined ? amountText(remaining, found.assetScale) : "",
    );
    setNote("");
    setStep("amount");
  };

  const getQuote = async () => {
    const found = recipient();
    if (!found || pending()) return;
    setError(undefined);
    let value: bigint | undefined;
    if (amount().trim() !== "") {
      const parsed = parseAmount(amount(), scale());
      if (!parsed.ok) return setError(parseFailureText(parsed.reason, scale()));
      if (parsed.value === 0n) return setError("Send more than nothing.");
      value = parsed.value;
    } else if (!found.request) {
      return setError("Enter an amount.");
    }
    setPending(true);
    try {
      // What was found, not what the box says now.
      // An amount to send is in the chosen ledger's asset, so it pays
      // from that one; else the node pays from the route's.
      const from = fixed() === "send" ? chosen()?.id : ledger();
      const response = await client.call("quoteSend", {
        recipient: found.request?.url ?? found.walletAddress,
        ledger: from ?? "",
        amount: quoteAmount(fixed(), value),
        note: note(),
      });
      setQuote(response.quote);
      setStep("review");
    } catch (failure) {
      setError(describe(failure));
    }
    setPending(false);
  };
  const toReview = (event: SubmitEvent) => {
    event.preventDefault();
    void getQuote();
  };

  const confirm = async () => {
    const quoted = quote();
    if (!quoted || pending()) return;
    setPending(true);
    setError(undefined);
    try {
      const response = await client.call("sendPayment", {
        quoteId: quoted.id,
      });
      if (response.payment) {
        router.navigate(`/payments/${response.payment.id}`);
        return;
      }
    } catch (failure) {
      setError(describe(failure));
    }
    setPending(false);
  };
  const lapsed = () => {
    const quoted = quote();
    return quoted !== undefined && Number(quoted.expiresAtUnixMs) <= now();
  };
  const back = (to: Step) => {
    setError(undefined);
    setStep(to);
  };

  return (
    <div class="page page-narrow stack" data-testid="page-send">
      <PageTitle>Send</PageTitle>
      <Show when={step() === "recipient"}>
        <Card title="Whom to Pay">
          <form class="stack" onSubmit={toAmount}>
            <Field
              label="Payment pointer, wallet address or payment request"
              for={`${id}-recipient`}
            >
              <input
                id={`${id}-recipient`}
                data-testid="send-recipient"
                placeholder="$wallet.example"
                autocomplete="off"
                spellcheck={false}
                value={input()}
                onInput={(event) => typed(event.currentTarget.value)}
              />
            </Field>
            <Show when={recipient()}>
              {(found) => <RecipientFacts recipient={found()} />}
            </Show>
            <Show when={pending()}>
              <p class="muted" role="status">Looking it up…</p>
            </Show>
            <div class="actions">
              <button
                type="submit"
                class="primary"
                data-testid="send-continue"
                disabled={pending() || input().trim() === ""}
              >
                Continue
              </button>
            </div>
            <ErrorText message={error()} />
          </form>
        </Card>
      </Show>
      <Show when={step() === "amount" && recipient()}>
        {(found) => (
          <Card title="How Much">
            <form class="stack" onSubmit={toReview}>
              <p>
                To{" "}
                <strong data-testid="send-to">{recipientName(found())}</strong>
              </p>
              <Show when={found().request?.description}>
                {(text) => <p class="muted">For: {text()}</p>}
              </Show>
              <Show
                when={ledgers().length > 0}
                fallback={
                  <p class="error" role="alert">
                    You have no ledger to pay from.
                  </p>
                }
              >
                <Show when={ledgers().length > 1}>
                  <Field label="Pay from" for={`${id}-ledger`}>
                    <select
                      id={`${id}-ledger`}
                      data-testid="send-ledger"
                      onChange={(event) => setLedger(event.currentTarget.value)}
                    >
                      <For each={ledgers()}>
                        {(entry) => (
                          <option
                            value={entry.id}
                            selected={entry.id === chosen()?.id}
                          >
                            {entry.id}
                          </option>
                        )}
                      </For>
                    </select>
                  </Field>
                </Show>
                <Show when={chosen()}>
                  {(entry) => (
                    <p class="muted">
                      You have{" "}
                      <Amount
                        value={entry().ownerBalance}
                        currency={ledgerCurrency(entry())}
                        testid="send-balance"
                      />.
                    </p>
                  )}
                </Show>
                <fieldset class="radios">
                  <legend>The amount is</legend>
                  <For
                    each={[
                      ["receive", "What they receive"],
                      ["send", "What you send"],
                    ] as const}
                  >
                    {([value, label]) => (
                      <label>
                        <input
                          type="radio"
                          name={`${id}-fixed`}
                          data-testid={`send-fixed-${value}`}
                          checked={fixed() === value}
                          onChange={() => {
                            setFixed(value);
                            setAmount("");
                          }}
                        />{" "}
                        {label}
                      </label>
                    )}
                  </For>
                </fieldset>
                <Field
                  label={amountLabel()}
                  for={`${id}-amount`}
                  hint={fixed() === "receive" &&
                      found().request?.remaining !== undefined
                    ? "What the request asks for; less pays part of it."
                    : fixed() === "send"
                    ? "They receive what it buys at the rate of the route."
                    : undefined}
                >
                  <input
                    id={`${id}-amount`}
                    inputmode="decimal"
                    data-testid="send-amount"
                    value={amount()}
                    onInput={(event) => setAmount(event.currentTarget.value)}
                  />
                </Field>
                <Show when={!found().request}>
                  <Field label="Note for them" for={`${id}-note`}>
                    <input
                      id={`${id}-note`}
                      data-testid="send-note"
                      maxlength="256"
                      value={note()}
                      onInput={(event) => setNote(event.currentTarget.value)}
                    />
                  </Field>
                </Show>
                <div class="actions">
                  <button
                    type="submit"
                    class="primary"
                    data-testid="send-review"
                    disabled={pending()}
                  >
                    Review
                  </button>
                  <button
                    type="button"
                    data-testid="send-back"
                    onClick={() => back("recipient")}
                  >
                    Back
                  </button>
                </div>
              </Show>
              <ErrorText message={error()} />
            </form>
          </Card>
        )}
      </Show>
      <Show when={step() === "review" && quote()}>
        {(quoted) => (
          <Card title="Review">
            <div class="stack">
              <dl class="facts">
                <dt>To</dt>
                <dd data-testid="send-review-to">
                  {recipientName(
                    quoted().recipient ?? {
                      publicName: "",
                      paymentPointer: "",
                      walletAddress: input(),
                    },
                  )}
                </dd>
                <dt>From</dt>
                <dd data-testid="send-review-ledger">{quoted().ledger}</dd>
                <dt>You send</dt>
                <dd>
                  <Amount
                    value={quoted().debitAmount}
                    currency={ledgerCurrency(quoted())}
                    testid="send-debit"
                  />
                </dd>
                <dt>They receive</dt>
                <dd>
                  <Amount
                    value={quoted().receiveAmount?.value ?? 0n}
                    currency={ledgerCurrency(
                      quoted().receiveAmount ?? quoted(),
                    )}
                    testid="send-receive"
                  />
                </dd>
                <Show when={note() !== ""}>
                  <dt>Note</dt>
                  <dd>{note()}</dd>
                </Show>
              </dl>
              <p class="muted" data-testid="send-lapse">
                {lapseText(quoted().expiresAtUnixMs, now())}
              </p>
              <div class="actions">
                <Show
                  when={!lapsed() && error() === undefined}
                  fallback={
                    <button
                      type="button"
                      class="primary"
                      data-testid="send-requote"
                      disabled={pending()}
                      onClick={() => void getQuote()}
                    >
                      Get a New Quote
                    </button>
                  }
                >
                  <button
                    type="button"
                    class="primary"
                    data-testid="send-confirm"
                    disabled={pending()}
                    onClick={() => void confirm()}
                  >
                    Send
                  </button>
                </Show>
                <button
                  type="button"
                  data-testid="send-back"
                  onClick={() => back("amount")}
                >
                  Back
                </button>
              </div>
              <ErrorText message={error()} />
            </div>
          </Card>
        )}
      </Show>
    </div>
  );
}

/** What the node found: whom it pays, in what, and what a request asks. */
function RecipientFacts(props: { recipient: Recipient }): JSX.Element {
  const currency = () => ledgerCurrency(props.recipient);
  return (
    <dl class="facts" data-testid="send-recipient-found">
      <dt>Recipient</dt>
      <dd data-testid="send-recipient-name">
        {recipientName(props.recipient)}
        <Show when={props.recipient.isOwn}>{` (one of yours)`}</Show>
      </dd>
      <dt>Currency</dt>
      <dd>{props.recipient.assetCode}</dd>
      <Show when={props.recipient.request}>
        {(request) => (
          <>
            <dt>Request</dt>
            <dd data-testid="send-request">
              <Show
                when={request().remaining !== undefined}
                fallback="any amount"
              >
                <Amount
                  value={request().remaining ?? 0n}
                  currency={currency()}
                />
              </Show>
              <Show when={request().description !== ""}>
                {` for ${request().description}`}
              </Show>
            </dd>
          </>
        )}
      </Show>
    </dl>
  );
}
