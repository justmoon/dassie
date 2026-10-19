/**
 * Receive (spec/areas/wallet.md#receive): the wallet addresses other wallets
 * pay through Open Payments, a request for a payment, and a tab per ledger with
 * a deposit method: the stub's test money.
 */

import type { JSX } from "@solidjs/web";
import type { Ledger } from "@dassie/rpc/proto";
import { createSignal, Show } from "solid-js";
import { formatAmount } from "@dassie/ui/currency";
import { ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Link, useRouter, useSearchParam } from "@dassie/ui/router";
import { useToast } from "@dassie/ui/toast";
import { Card, ErrorText, PageTitle, Tabs } from "@dassie/ui/ui";
import { schemeName } from "./schemes.ts";
import { RequestPayment } from "./payments/RequestPayment.tsx";
import { WalletAddresses } from "./payments/WalletAddresses.tsx";

export function Receive(): JSX.Element {
  const data = useNodeData();
  const tab = useSearchParam("tab");
  const [selected, setSelected] = createSignal(tab() ?? "open-payments");
  const tabs = () => [
    {
      id: "open-payments",
      title: "Open Payments",
      panel: () => (
        <div class="stack">
          <WalletAddresses />
          <RequestPayment />
        </div>
      ),
    },
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
