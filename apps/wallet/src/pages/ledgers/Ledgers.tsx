/** Ledgers (spec 6 §1.6.7): each active ledger, with its balances. */

import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import { Amount, ledgerCurrency } from "../../shared/amount/Amount.tsx";
import { useNodeData } from "../../shared/node-data.ts";
import { Link } from "../../shared/router/Router.tsx";
import { Card, LiveNote, PageTitle } from "../../shared/ui/ui.tsx";
import { schemeName } from "../schemes.ts";

export function Ledgers(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>Active Ledgers</PageTitle>
      <LiveNote status={data.ledgersStatus()} />
      <For each={data.ledgers() ?? []} keyed={(ledger) => ledger.id}>
        {(ledger) => (
          <Card testid={`ledger-${ledger().id}`} class="ledger">
            <div class="ledger-head">
              <h2>{schemeName(ledger().settlementScheme)}</h2>
              <Link
                href={`/ledgers/delete/${ledger().settlementScheme}`}
                class="button danger icon"
                testid={`ledger-delete-${ledger().settlementScheme}`}
                aria-label={`Remove ${schemeName(ledger().settlementScheme)}`}
              >
                🗑
              </Link>
            </div>
            <Amount
              value={ledger().ownerBalance}
              currency={ledgerCurrency(ledger())}
              size="lg"
              testid={`ledger-balance-${ledger().id}`}
            />
            <p class="muted">
              On the underlying ledger:{" "}
              <Amount
                value={ledger().onLedgerBalance}
                currency={ledgerCurrency(ledger())}
              />
            </p>
            <div class="actions">
              <Link
                href={`/receive?tab=${ledger().settlementScheme}`}
                class="button"
                testid={`ledger-receive-${ledger().settlementScheme}`}
              >
                Receive
              </Link>
            </div>
          </Card>
        )}
      </For>
      <div class="actions">
        <Link href="/ledgers/create" class="button" testid="add-ledger">
          + Add Ledger
        </Link>
      </div>
    </div>
  );
}
