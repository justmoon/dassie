/**
 * Receive (spec/areas/wallet.md#pages): the wallet addresses other wallets
 * pay through Open Payments, one per asset, a request for a payment, and a
 * tab per asset with its deposit method, as the node reports it for the
 * asset's wallet address: the stub's test money, or the address of the
 * owner's own account, never a settlement hub's escrow
 * (spec/decisions/one-balance-per-asset.md), with the testnet faucet where
 * the node has one. A ledger that settles in a token names both what its
 * account needs there: the token, and the ledger's own asset for fees.
 */

import type { JSX } from "@solidjs/web";
import { createEffect, createSignal } from "solid-js";
import { useNodeData } from "@dassie/ui/node-data";
import { Link, useSearchParam } from "@dassie/ui/router";
import { Card, PageTitle, Tabs } from "@dassie/ui/ui";
import { schemeName } from "./schemes.ts";
import { assetTab } from "./receive.ts";
import { RequestPayment } from "./payments/RequestPayment.tsx";
import {
  useWalletAddresses,
  WalletAddresses,
} from "./payments/WalletAddresses.tsx";
import { Deposit } from "./ledgers/Deposit.tsx";

export function Receive(): JSX.Element {
  const data = useNodeData();
  const addresses = useWalletAddresses();
  const tab = useSearchParam("tab");
  const [selected, setSelected] = createSignal(tab() ?? "open-payments");
  // Where a deposit goes follows the node's ledgers: a ledger added or
  // removed is asked for again.
  createEffect(
    () => data.ledgers()?.map((ledger) => ledger.id).join(" "),
    () => void addresses.reload(),
  );
  const deposits = () =>
    (addresses.addresses() ?? []).flatMap((address) => {
      const ledger = data.ledgers()?.find((entry) =>
        entry.id === address.depositLedger
      );
      return ledger ? [ledger] : [];
    });
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
    ...deposits().map((ledger) => ({
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
          selected={assetTab(selected(), tabs().map((entry) => entry.id))}
          onSelect={setSelected}
        />
        <div class="actions">
          <Link href="/" class="button" testid="receive-back">Back</Link>
        </div>
      </Card>
    </div>
  );
}
