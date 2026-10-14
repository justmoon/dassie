/**
 * Receive (spec 6 §1.6.6): the payment pointer, STREAM credentials until
 * Open Payments arrives (M6), and a tab per ledger with a deposit method:
 * the stub's test money.
 */

import type { JSX } from "@solidjs/web";
import type {
  CreateStreamCredentialsResponse,
  Ledger,
} from "@dassie/rpc/proto";
import { createSignal, Show } from "solid-js";
import { formatAmount } from "../shared/amount/currency.ts";
import { ledgerCurrency } from "../shared/amount/Amount.tsx";
import { hex } from "../shared/format.ts";
import { useNodeData } from "../shared/node-data.ts";
import { describe, useClient } from "../shared/rpc.ts";
import { Link, useRouter, useSearchParam } from "../shared/router/Router.tsx";
import { useToast } from "../shared/ui/toast.tsx";
import {
  Card,
  CopyButton,
  ErrorText,
  Masked,
  PageTitle,
  Tabs,
} from "../shared/ui/ui.tsx";
import { schemeName } from "./schemes.ts";

export function Receive(): JSX.Element {
  const data = useNodeData();
  const tab = useSearchParam("tab");
  const [selected, setSelected] = createSignal(tab() ?? "interledger");
  const tabs = () => [
    { id: "interledger", title: "Interledger", panel: () => <Interledger /> },
    ...(data.ledgers() ?? []).map((ledger) => ({
      id: ledger.settlementScheme,
      title: schemeName(ledger.settlementScheme),
      panel: () => <Deposit ledger={ledger} />,
    })),
  ];
  return (
    <div class="page-narrow">
      <PageTitle>Receive</PageTitle>
      <Card testid="page-receive">
        <Tabs
          label="How to receive"
          tabs={tabs()}
          selected={selected()}
          onSelect={setSelected}
        />
        <div class="actions">
          <Link href="/" class="button" testid="receive-back">Back</Link>
        </div>
      </Card>
    </div>
  );
}

function Interledger(): JSX.Element {
  const data = useNodeData();
  const client = useClient();
  const [credentials, setCredentials] = createSignal<
    CreateStreamCredentialsResponse
  >();
  const [error, setError] = createSignal<string>();
  const show = async () => {
    setError(undefined);
    try {
      setCredentials(
        await client.call("createStreamCredentials", { tag: "wallet" }),
      );
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <div class="stack">
      <p>Send money to the following payment pointer:</p>
      <p class="pointer boxed" data-testid="receive-pointer">
        {data.hostname ? `$${data.hostname}` : "???"}
      </p>
      <p class="muted">
        Other wallets can pay this address once your node supports Open
        Payments. Until then, a STREAM sender can pay your node with these
        credentials.
      </p>
      <Show
        when={credentials()}
        fallback={
          <button type="button" data-testid="stream-credentials" onClick={show}>
            Show STREAM credentials
          </button>
        }
      >
        {(c) => (
          <dl class="facts">
            <dt>Destination</dt>
            <dd>
              <code data-testid="stream-destination">
                {c().destinationAccount}
              </code>
              <CopyButton text={c().destinationAccount} />
            </dd>
            <dt>Shared secret</dt>
            <dd>
              <Masked text={hex(c().sharedSecret)} testid="stream-secret" />
              <CopyButton text={hex(c().sharedSecret)} />
            </dd>
            <dt>Asset</dt>
            <dd>{c().assetCode}, scale {c().assetScale}</dd>
          </dl>
        )}
      </Show>
      <ErrorText message={error()} />
    </div>
  );
}

function Deposit(props: { ledger: Ledger }): JSX.Element {
  return (
    <Show
      when={props.ledger.settlementScheme === "stub"}
      fallback={<p class="muted">No deposit method for this ledger yet.</p>}
    >
      <StubDeposit ledger={props.ledger} />
    </Show>
  );
}

/** The stub's test money: $100 at a click. */
function StubDeposit(props: { ledger: Ledger }): JSX.Element {
  const client = useClient();
  const router = useRouter();
  const toast = useToast();
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const amount = () => 100n * 10n ** BigInt(props.ledger.assetScale);
  const receive = async () => {
    setPending(true);
    setError(undefined);
    try {
      await client.call("stubDeposit", { amount: amount() });
      toast({
        title: `Received ${
          formatAmount(amount(), ledgerCurrency(props.ledger)).text
        }`,
      });
      router.navigate("/");
    } catch (failure) {
      setError(describe(failure));
      setPending(false);
    }
  };
  return (
    <div class="stack">
      <p>
        The stub ledger is not a real ledger: it hands out test money. Click
        below to receive $100 into your stub account.
      </p>
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-testid="stub-deposit"
          disabled={pending()}
          onClick={receive}
        >
          Receive $100
        </button>
      </div>
      <ErrorText message={error()} />
    </div>
  );
}
