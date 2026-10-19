/**
 * Home (spec/areas/wallet.md#pages): the payment pointer, and the owner's
 * balance on each ledger in its own currency (UI-2), as it changes, with the
 * total in the display currency (D6) while the rates allow.
 */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { useWalletAddresses } from "./payments/WalletAddresses.tsx";
import { Card, LiveNote, PageTitle } from "@dassie/ui/ui";
import { schemeName } from "./schemes.ts";

export function Account(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>Dashboard</PageTitle>
      <PaymentPointer />
      <Card title="Balance" testid="page-account">
        <LiveNote status={data.ledgersStatus()} />
        <ul class="balances">
          <For each={data.ledgers() ?? []} keyed={(ledger) => ledger.id}>
            {(ledger) => (
              <li>
                <span class="muted">
                  {schemeName(ledger().settlementScheme)}
                </span>
                <Amount
                  value={ledger().ownerBalance}
                  currency={ledgerCurrency(ledger())}
                  size="lg"
                  testid={`balance-${ledger().id}`}
                />
                <Show when={ledger().ownerPending > 0n}>
                  <span class="muted">
                    (<Amount
                      value={ledger().ownerPending}
                      currency={ledgerCurrency(ledger())}
                    />{" "}
                    in flight)
                  </span>
                </Show>
              </li>
            )}
          </For>
        </ul>
        <Show when={data.total()}>
          {(total) => (
            <p class="muted">
              In all:{" "}
              <Amount
                value={total().amount}
                currency={ledgerCurrency(total())}
                testid="balance-total"
              />
            </p>
          )}
        </Show>
        <div class="actions">
          <Link href="/send" class="button primary" testid="send-button">
            Send
          </Link>
          <Link href="/receive" class="button primary" testid="receive-button">
            Receive
          </Link>
        </div>
      </Card>
    </div>
  );
}

/** The node's payment pointer: its default wallet address, `$hostname`. */
export function PaymentPointer(props: { testid?: string }): JSX.Element {
  const data = useNodeData();
  const state = useWalletAddresses();
  const pointer = () =>
    state.addresses()?.find((address) => address.isDefault)?.paymentPointer ??
      (data.hostname ? `$${data.hostname}` : "???");
  return (
    <Card title="Payment Pointer">
      <p class="pointer" data-testid={props.testid ?? "payment-pointer"}>
        {pointer()}
      </p>
      <p class="muted" data-testid="pointer-note">
        Other wallets pay this address through Open Payments.
      </p>
    </Card>
  );
}
