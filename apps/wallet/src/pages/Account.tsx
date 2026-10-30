/**
 * Home (spec/areas/wallet.md#pages): the owner's balance, the total in the
 * display currency (spec/decisions/owner-ledger.md) while the rates allow
 * and below it each asset in its own currency, as it changes, its
 * settlement hub's and its own account's together, reserves included
 * (spec/decisions/one-balance-per-asset.md), what a payment may spend of
 * it where that is less, and a link to its own page; Send and Receive; the
 * last few payments; and the payment pointer.
 *
 * It is the top of the wallet in Kopje's brand page
 * (apps/website/brand/index.html, "Node wallet"): the balance on a
 * Terracotta card with the veld's grass along its foot, the brand page's
 * own drawing, checked against it (`drawings_test.ts`).
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
import { ReceiveIcon, SendIcon } from "./payments/icons.tsx";
import { Recent } from "./payments/Recent.tsx";
import { Card, LiveNote, PageTitle } from "@dassie/ui/ui";
import { showsSpendable } from "./assets/assets.ts";

const GRASS = new URL("./balance-grass.svg", import.meta.url).href;

export function Account(): JSX.Element {
  return (
    <div class="page-wide home">
      <PageTitle>Dashboard</PageTitle>
      <Balance />
      <div class="home-actions">
        <Link href="/send" class="button primary" testid="send-button">
          <SendIcon />
          Send
        </Link>
        <Link href="/receive" class="button" testid="receive-button">
          <ReceiveIcon />
          Receive
        </Link>
      </div>
      <Recent />
      <PaymentPointer />
    </div>
  );
}

/** The balance card: the total, big, and each asset below it. Without a
 * total, each asset's balance is the big number. */
function Balance(): JSX.Element {
  const data = useNodeData();
  return (
    <section class="balance-card" data-testid="page-account">
      <div>
        <h2 class="overline">Balance</h2>
        <LiveNote status={data.ledgersStatus()} />
        <Show when={data.total()}>
          {(total) => (
            <p class="balance-total">
              <Amount
                value={total().amount}
                currency={ledgerCurrency(total())}
                testid="balance-total"
              />
            </p>
          )}
        </Show>
        <ul class="balances">
          <For each={data.balances()} keyed={(balance) => balance.name}>
            {(balance) => (
              <li>
                <Link
                  href={`/assets/${balance().name}`}
                  class="balance-row"
                  testid={`balance-open-${balance().name}`}
                >
                  <span class="balance-asset">
                    {assetLabel(balance().name)}
                  </span>
                  <Amount
                    value={balance().ownerBalance}
                    currency={ledgerCurrency(balance())}
                    size={data.total() ? "md" : "lg"}
                    testid={`balance-${balance().name}`}
                  />
                  <Show when={balance().ownerPending > 0n}>
                    <span class="balance-note">
                      (<Amount
                        value={balance().ownerPending}
                        currency={ledgerCurrency(balance())}
                      />{" "}
                      in flight)
                    </span>
                  </Show>
                  <Show when={showsSpendable(balance())}>
                    <span
                      class="balance-note"
                      data-testid={`balance-spendable-${balance().name}`}
                    >
                      <Amount
                        value={balance().spendable}
                        currency={ledgerCurrency(balance())}
                      />{" "}
                      spendable
                    </span>
                  </Show>
                  <span class="balance-chevron" aria-hidden="true">›</span>
                </Link>
              </li>
            )}
          </For>
        </ul>
      </div>
      <img class="drawn" src={GRASS} alt="" />
    </section>
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
    <Card title="Payment pointer" class="home-pointer">
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
