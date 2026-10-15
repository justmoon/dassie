/** Ledgers (spec 6 §1.6.7): each active ledger, with its balances. */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Qr } from "@dassie/ui/qr";
import { Link } from "@dassie/ui/router";
import { Card, CopyButton, LiveNote, PageTitle } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { connectionText } from "./connection.ts";

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
            <Show when={connectionText(ledger())}>
              {(text) => (
                <p data-testid={`ledger-connection-${ledger().id}`}>
                  {text()}
                </p>
              )}
            </Show>
            <Show when={ledger().depositAddress}>
              {(address) => (
                <div class="deposit">
                  <Qr
                    text={address()}
                    label={`Deposit address ${address()}`}
                    testid={`ledger-qr-${ledger().id}`}
                  />
                  <div>
                    <p>Deposit address</p>
                    <p>
                      <code data-testid={`ledger-address-${ledger().id}`}>
                        {address()}
                      </code>{" "}
                      <CopyButton text={address()} label="Copy address" />
                    </p>
                    <Show when={ledger().reserve > 0n}>
                      <p class="muted">
                        The ledger holds back{" "}
                        <Amount
                          value={ledger().reserve}
                          currency={ledgerCurrency(ledger())}
                        />{" "}
                        as a reserve: never spent, and not in the balance.
                      </p>
                    </Show>
                  </div>
                </div>
              )}
            </Show>
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
