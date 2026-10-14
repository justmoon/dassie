/**
 * Home (spec 6 §1.6.4): the payment pointer, and the owner's balance on
 * each ledger in its own currency (UI-2), as it changes.
 */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "../shared/amount/Amount.tsx";
import { useNodeData } from "../shared/node-data.ts";
import { Link } from "../shared/router/Router.tsx";
import { Card, LiveNote, PageTitle } from "../shared/ui/ui.tsx";
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
          <For each={data.ledgers() ?? []}>
            {(ledger) => (
              <li>
                <span class="muted">{schemeName(ledger.settlementScheme)}</span>
                <Amount
                  value={ledger.ownerBalance}
                  currency={ledgerCurrency(ledger)}
                  size="lg"
                  testid={`balance-${ledger.id}`}
                />
                <Show when={ledger.ownerPending > 0n}>
                  <span class="muted">
                    (<Amount
                      value={ledger.ownerPending}
                      currency={ledgerCurrency(ledger)}
                    />{" "}
                    in flight)
                  </span>
                </Show>
              </li>
            )}
          </For>
        </ul>
        <div class="actions">
          <Link href="/receive" class="button primary" testid="receive-button">
            Receive
          </Link>
        </div>
      </Card>
    </div>
  );
}

/** The node's payment pointer, `$hostname`. */
export function PaymentPointer(props: { testid?: string }): JSX.Element {
  const data = useNodeData();
  return (
    <Card title="Payment Pointer">
      <p class="pointer" data-testid={props.testid ?? "payment-pointer"}>
        {data.hostname ? `$${data.hostname}` : "???"}
      </p>
      <p class="muted" data-testid="pointer-note">
        Other wallets can pay this address once your node supports Open
        Payments.
      </p>
    </Card>
  );
}
