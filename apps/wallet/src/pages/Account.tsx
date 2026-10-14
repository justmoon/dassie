/**
 * Home (spec 6 §1.6.4): the payment pointer, and the owner's balance on
 * each ledger in its own currency (UI-2), as it changes.
 */

import type { JSX } from "@solidjs/web";
import { createMemo, For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { shortId } from "@dassie/ui/format";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { useWalletAddresses } from "./payments/WalletAddresses.tsx";
import { Card, LiveNote, PageTitle } from "@dassie/ui/ui";
import { schemeName } from "./schemes.ts";
import { banText, recentAutomaticBans } from "./network/grace.ts";

export function Account(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>Dashboard</PageTitle>
      <RecentBans />
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

/** Peers the node banned lately for what they cost (D18): the owner is
 * told, and can look. */
function RecentBans(): JSX.Element {
  const data = useNodeData();
  const bans = createMemo(() =>
    recentAutomaticBans(Object.values(data.nodes.byId), Date.now())
  );
  return (
    <Show when={bans().length > 0}>
      <Card title="Peers Banned" testid="recent-bans">
        <ul>
          <For each={bans()}>
            {(node) => (
              <li>
                <Link href={`/network/${node.id}`}>
                  {node.alias || shortId(node.id, 20)}
                </Link>
                : {banText(node.ban!)}
              </li>
            )}
          </For>
        </ul>
      </Card>
    </Show>
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
