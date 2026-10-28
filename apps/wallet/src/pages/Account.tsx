/**
 * Home (spec/areas/wallet.md#pages): the payment pointer, and the owner's
 * balance in each asset in its own currency, as it changes, its settlement
 * hub's and its own account's together
 * (spec/decisions/one-balance-per-asset.md), with the total in the display
 * currency (spec/decisions/owner-ledger.md) while the rates allow.
 */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import {
  assetLabel,
  FrontDoorNote,
  useWalletAddresses,
} from "./payments/WalletAddresses.tsx";
import { Card, LiveNote, PageTitle } from "@dassie/ui/ui";

export function Account(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>Dashboard</PageTitle>
      <PaymentPointer />
      <Card title="Balance" testid="page-account">
        <LiveNote status={data.ledgersStatus()} />
        <ul class="balances">
          <For each={data.balances()} keyed={(balance) => balance.name}>
            {(balance) => (
              <li>
                <span class="muted">{assetLabel(balance().name)}</span>
                <Amount
                  value={balance().ownerBalance}
                  currency={ledgerCurrency(balance())}
                  size="lg"
                  testid={`balance-${balance().name}`}
                />
                <Show when={balance().ownerPending > 0n}>
                  <span class="muted">
                    (<Amount
                      value={balance().ownerPending}
                      currency={ledgerCurrency(balance())}
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
      <FrontDoorNote pointer={pointer()} />
    </Card>
  );
}
