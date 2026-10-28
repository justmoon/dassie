/** Ledgers (spec/areas/wallet.md#pages): each active ledger, with its
 * balances, whether it is being removed, and, for a dollar token's, its peg
 * guard, and the exchange rates the node converts at, with their age
 * (spec/decisions/exchange-rates.md). */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { Alert, Card, LiveNote, PageTitle } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { assetName, assetOf } from "../assets/assets.ts";
import { ageText, rateText } from "../settings/rates.ts";
import { connectionText } from "./connection.ts";
import { DepositAddress } from "./DepositAddress.tsx";
import { PegGuard } from "./PegGuard.tsx";
import { removingNotice } from "./removal.ts";
import { assetLabel } from "../payments/WalletAddresses.tsx";

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
            <Show
              when={data.balances().find((balance) =>
                balance.ledgers.length > 1 &&
                balance.ledgers.includes(ledger().id)
              )}
            >
              {(balance) => (
                <p class="muted" data-testid={`ledger-part-${ledger().id}`}>
                  Part of your {assetLabel(balance().name)},{" "}
                  <Amount
                    value={balance().ownerBalance}
                    currency={ledgerCurrency(balance())}
                  />{" "}
                  in all.
                </p>
              )}
            </Show>
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
            <Show when={removingNotice(ledger().removingMethods)}>
              {(notice) => (
                <Alert
                  title={notice().title}
                  testid={`ledger-removing-${ledger().id}`}
                >
                  {notice().text}
                </Alert>
              )}
            </Show>
            <PegGuard ledger={ledger()} />
            <Show
              when={assetOf(ledger().settlementScheme, data.externalAssets())}
            >
              {(asset) => (
                <p class="muted">
                  Settles through your{" "}
                  <Link
                    href="/assets"
                    testid={`ledger-asset-${ledger().id}`}
                  >
                    {assetName(asset().id)}
                  </Link>
                  , which holds what the ledger reports.
                </p>
              )}
            </Show>
            <Show when={ledger().depositAddress}>
              {(address) => (
                <DepositAddress
                  ledger={ledger()}
                  address={address()}
                  testid="ledger"
                  reserve
                />
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
      <Show when={data.rates().some((rate) => rate.assetCode !== "USD")}>
        <Card title="Exchange Rates" testid="rates">
          <ul>
            <For
              each={data.rates().filter((rate) => rate.assetCode !== "USD")}
            >
              {(rate) => (
                <li
                  data-testid={`rate-${rate.assetCode}`}
                  data-stale={rate.stale ? "yes" : "no"}
                >
                  {rateText(rate.assetCode, rate.perUsd)}{" "}
                  <span class="muted">
                    ({ageText(rate.asOfUnixMs, Date.now(), rate.stale)})
                  </span>
                </li>
              )}
            </For>
          </ul>
          <p class="muted">
            Change their sources in <Link href="/settings">Settings</Link>.
          </p>
        </Card>
      </Show>
    </div>
  );
}
