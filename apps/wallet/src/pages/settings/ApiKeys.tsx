/**
 * BTP tokens (spec 6 §1.6.12, "API Keys"): each with its ledger, what went
 * through it, and its spending cap (spec 7 §4.5). A new token's secret is
 * shown once, in a dialog, and never again: the node keeps only its hash.
 */

import type { JSX } from "@solidjs/web";
import type { BtpToken, CreateBtpTokenResponse } from "@dassie/rpc/proto";
import {
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  For,
  Show,
} from "solid-js";
import { Amount, ledgerCurrency } from "../../shared/amount/Amount.tsx";
import { currencyOf, parseAmount } from "../../shared/amount/currency.ts";
import { dateTime } from "../../shared/format.ts";
import { useNodeData } from "../../shared/node-data.ts";
import { describe, useClient } from "../../shared/rpc.ts";
import { useToast } from "../../shared/ui/toast.tsx";
import {
  Card,
  ConfirmButton,
  CopyButton,
  ErrorText,
  Field,
  Masked,
} from "../../shared/ui/ui.tsx";
import { formatCap, usedShare, WINDOWS } from "./caps.ts";

export function ApiKeys(): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const [version, setVersion] = createSignal(0);
  const tokens = createMemo(async () => {
    version();
    return (await client.call("listBtpTokens", {})).tokens;
  });
  const [created, setCreated] = createSignal<CreateBtpTokenResponse>();
  const refresh = () => setVersion((n) => n + 1);
  const currency = (ledger: string) => {
    const known = data.ledgers()?.find((entry) => entry.id === ledger);
    return known ? ledgerCurrency(known) : currencyOf("?", 9);
  };
  return (
    <Card title="API Keys" testid="api-keys">
      <p class="muted">Manage BTP access</p>
      <Show
        when={tokens().length > 0}
        fallback={<p class="muted">No API keys created yet. Add one below.</p>}
      >
        <ul class="tokens">
          <For each={tokens()}>
            {(token) => (
              <TokenRow
                token={token}
                currency={currency(token.ledger)}
                onChange={refresh}
              />
            )}
          </For>
        </ul>
      </Show>
      <CreateToken
        onCreated={(response) => {
          setCreated(response);
          refresh();
        }}
      />
      <Show when={created()}>
        {(response) => (
          <NewToken
            response={response()}
            onClose={() => setCreated(undefined)}
          />
        )}
      </Show>
    </Card>
  );
}

function TokenRow(props: {
  token: BtpToken;
  currency: ReturnType<typeof currencyOf>;
  onChange: () => void;
}): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const token = () => props.token;
  return (
    <li data-testid={`btp-token-${token().id}`}>
      <div class="token-head">
        <strong>{token().label || "unnamed"}</strong>{" "}
        <span class="muted">{token().ledger}</span>
      </div>
      <div class="muted">
        made {dateTime(token().createdAtUnixMs)}, {token().connections}{" "}
        connected; received net{" "}
        <Amount value={token().netReceived} currency={props.currency} />
      </div>
      <div data-testid={`btp-token-cap-${token().id}`}>
        Cap: {formatCap(token().cap, props.currency)}
        <Show when={token().cap}>
          {(cap) => (
            <>
              {" "}
              <meter
                min="0"
                max="1"
                value={usedShare(cap(), token().capAvailable)}
                aria-label="Share of the cap used"
              />{" "}
              <span class="muted">
                <Amount
                  value={token().capAvailable}
                  currency={props.currency}
                />{" "}
                left
              </span>
            </>
          )}
        </Show>
      </div>
      <div class="actions">
        <CapEditor
          token={token()}
          currency={props.currency}
          onChange={props.onChange}
        />
        <ConfirmButton
          label="Revoke"
          title="Revoke this token?"
          confirm="Revoke"
          tone="danger"
          testid={`revoke-token-${token().id}`}
          onConfirm={async () => {
            try {
              await client.call("revokeBtpToken", { id: token().id });
              toast({ title: "Token revoked" });
              props.onChange();
            } catch (failure) {
              toast({
                title: "That did not work",
                description: describe(failure),
                tone: "destructive",
              });
            }
          }}
        >
          <p>Its clients are disconnected, and it is refused from now on.</p>
        </ConfirmButton>
      </div>
    </li>
  );
}

/** A cap's amount and window, as inputs. */
function CapInputs(props: {
  id: string;
  amount: string;
  window: string;
  onAmount: (text: string) => void;
  onWindow: (id: string) => void;
}): JSX.Element {
  return (
    <div class="cap-inputs">
      <Field
        label="Spending cap"
        for={`${props.id}-cap`}
        hint="Leave empty for no limit. Paying your own node does not count."
      >
        <input
          id={`${props.id}-cap`}
          inputmode="decimal"
          data-testid="token-cap-amount"
          placeholder="no limit"
          value={props.amount}
          onInput={(event) => props.onAmount(event.currentTarget.value)}
        />
      </Field>
      <Field label="Per" for={`${props.id}-window`}>
        <select
          id={`${props.id}-window`}
          data-testid="token-cap-window"
          onChange={(event) => props.onWindow(event.currentTarget.value)}
        >
          <For each={WINDOWS}>
            {(window) => (
              <option value={window.id} selected={window.id === props.window}>
                {window.label.replace("per ", "")}
              </option>
            )}
          </For>
        </select>
      </Field>
    </div>
  );
}

/** The cap from the inputs: none, a cap, or why it is not one. */
function readCap(
  amount: string,
  window: string,
  scale: number,
): { cap?: { amount: bigint; windowMs: bigint }; error?: string } {
  if (amount.trim() === "") return {};
  const parsed = parseAmount(amount, scale);
  if (!parsed.ok || parsed.value === 0n) {
    return { error: "The cap must be an amount above zero." };
  }
  const ms = WINDOWS.find((entry) => entry.id === window)?.ms ?? WINDOWS[1].ms;
  return { cap: { amount: parsed.value, windowMs: ms } };
}

function CreateToken(props: {
  onCreated: (response: CreateBtpTokenResponse) => void;
}): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const id = createUniqueId();
  const [label, setLabel] = createSignal("");
  const [ledger, setLedger] = createSignal<string>();
  const [amount, setAmount] = createSignal("");
  const [window, setWindow] = createSignal("day");
  const [error, setError] = createSignal<string>();
  const chosen = () => ledger() ?? data.ledgers()?.[0]?.id ?? "";
  const create = async (event: SubmitEvent) => {
    event.preventDefault();
    setError(undefined);
    const scale = data.ledgers()?.find((l) => l.id === chosen())?.assetScale ??
      9;
    const cap = readCap(amount(), window(), scale);
    if (cap.error) {
      setError(cap.error);
      return;
    }
    try {
      props.onCreated(
        await client.call("createBtpToken", {
          label: label(),
          ledger: chosen(),
          cap: cap.cap,
        }),
      );
      setLabel("");
      setAmount("");
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <form class="create-token" onSubmit={create}>
      <h3>Create BTP Token</h3>
      <Field label="Label" for={`${id}-label`}>
        <input
          id={`${id}-label`}
          data-testid="token-label"
          maxlength="64"
          value={label()}
          onInput={(event) => setLabel(event.currentTarget.value)}
        />
      </Field>
      <Field label="Ledger" for={`${id}-ledger`}>
        <select
          id={`${id}-ledger`}
          data-testid="token-ledger"
          onChange={(event) => setLedger(event.currentTarget.value)}
        >
          <For each={data.ledgers() ?? []}>
            {(entry) => (
              <option value={entry.id} selected={entry.id === chosen()}>
                {entry.id}
              </option>
            )}
          </For>
        </select>
      </Field>
      <CapInputs
        id={id}
        amount={amount()}
        window={window()}
        onAmount={setAmount}
        onWindow={setWindow}
      />
      <div class="actions">
        <button type="submit" class="primary" data-testid="create-token">
          Create BTP Token
        </button>
      </div>
      <ErrorText message={error()} testid="create-token-error" />
    </form>
  );
}

function CapEditor(props: {
  token: BtpToken;
  currency: ReturnType<typeof currencyOf>;
  onChange: () => void;
}): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const id = createUniqueId();
  const [open, setOpen] = createSignal(false);
  const [amount, setAmount] = createSignal("");
  const [window, setWindow] = createSignal("day");
  const [error, setError] = createSignal<string>();
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    const cap = readCap(amount(), window(), props.currency.scale);
    if (cap.error) {
      setError(cap.error);
      return;
    }
    try {
      await client.call("setBtpTokenCap", { id: props.token.id, cap: cap.cap });
      toast({ title: cap.cap ? "Cap set" : "Cap lifted" });
      setOpen(false);
      props.onChange();
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <Show
      when={open()}
      fallback={
        <button
          type="button"
          data-testid={`edit-cap-${props.token.id}`}
          onClick={() => setOpen(true)}
        >
          Edit cap
        </button>
      }
    >
      <form class="cap-editor" onSubmit={save}>
        <CapInputs
          id={id}
          amount={amount()}
          window={window()}
          onAmount={setAmount}
          onWindow={setWindow}
        />
        <div class="actions">
          <button type="button" onClick={() => setOpen(false)}>Cancel</button>
          <button type="submit" class="primary" data-testid="save-cap">
            Save cap
          </button>
        </div>
        <ErrorText message={error()} />
      </form>
    </Show>
  );
}

/** The new token's secret and URL, once. */
function NewToken(props: {
  response: CreateBtpTokenResponse;
  onClose: () => void;
}): JSX.Element {
  const toast = useToast();
  let dialog: HTMLDialogElement | undefined;
  // Effects run once the dialog is in the document.
  createEffect(() => props.response, () => {
    if (dialog && !dialog.open) dialog.showModal();
  });
  return (
    <dialog
      ref={dialog}
      data-testid="new-token"
      onClose={() => props.onClose()}
    >
      <h2>Copy this token now. It will not be shown again.</h2>
      <dl class="facts">
        <dt>Token</dt>
        <dd>
          <Masked text={props.response.secret} testid="new-token-secret" />
        </dd>
        <dt>URL</dt>
        <dd>
          <Masked text={props.response.url} testid="new-token-url" />
        </dd>
      </dl>
      <div class="actions">
        <CopyButton
          text={props.response.url}
          label="Copy BTP URL"
          testid="new-token-copy"
          onCopied={(ok) =>
            toast(
              ok
                ? { title: "Copied BTP URL to clipboard" }
                : { title: "Could not copy", tone: "destructive" },
            )}
        />
        <button
          type="button"
          class="primary"
          data-testid="new-token-close"
          onClick={() => dialog?.close()}
        >
          Done
        </button>
      </div>
    </dialog>
  );
}
