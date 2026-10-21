/**
 * Receive (spec/areas/wallet.md#pages): the wallet addresses other wallets
 * pay through Open Payments, a request for a payment, and a tab per ledger with
 * its deposit method: the stub's test money, or the address the owner sends
 * to, with the testnet faucet where the node has one. A ledger that settles
 * in a token names both what its account needs there: the token, and the
 * ledger's own asset for fees.
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
import { schemeName, tokenScheme } from "./schemes.ts";
import { assetOf } from "./assets/assets.ts";
import { RequestPayment } from "./payments/RequestPayment.tsx";
import { WalletAddresses } from "./payments/WalletAddresses.tsx";
import { LedgerDeposit } from "./ledgers/LedgerDeposit.tsx";
import { TokenDeposit } from "./ledgers/TokenDeposit.tsx";

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
      panel: () => <Deposit id={ledger.id} />,
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

/** The panel stays while its tab does: it follows the ledger as it changes,
 * funded or topped up. */
function Deposit(props: { id: string }): JSX.Element {
  const data = useNodeData();
  const live = () => data.ledgers()?.find((ledger) => ledger.id === props.id);
  return (
    <Show when={live()}>
      {(ledger) => (
        <Show
          when={ledger().settlementScheme === "stub"}
          fallback={
            <Show
              when={ledger().depositAddress}
              fallback={
                <p class="muted">No deposit method for this ledger yet.</p>
              }
            >
              {(address) => (
                <Show
                  when={tokenScheme(ledger().settlementScheme)}
                  fallback={
                    <LedgerDeposit ledger={ledger()} address={address()} />
                  }
                >
                  {(token) => (
                    <TokenDeposit
                      ledger={ledger()}
                      address={address()}
                      token={token()}
                      asset={assetOf(
                        ledger().settlementScheme,
                        data.externalAssets(),
                      )}
                    />
                  )}
                </Show>
              )}
            </Show>
          }
        >
          <StubDeposit ledger={ledger()} />
        </Show>
      )}
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
