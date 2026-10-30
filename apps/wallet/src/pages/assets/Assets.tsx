/**
 * Assets (spec/areas/wallet.md#pages): each asset the owner holds, with
 * its one balance (spec/decisions/one-balance-per-asset.md), what a
 * payment may spend of it where that is less, how its ledgers stand, and
 * a link to its own page; a way to add another; and the exchange rates
 * the node converts at, with their age (spec/decisions/exchange-rates.md).
 */

import type { JSX } from "@solidjs/web";
import type { AssetBalance } from "@dassie/rpc/proto";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { Card, LiveNote, PageTitle, ValueCube } from "@dassie/ui/ui";
import { ageText, rateText } from "../settings/rates.ts";
import { connectionText } from "../ledgers/connection.ts";
import { assetTitle, showsSpendable } from "./assets.ts";

export function Assets(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>Assets</PageTitle>
      <LiveNote status={data.ledgersStatus()} />
      <div class="asset-list">
        <For each={data.balances()} keyed={(balance) => balance.name}>
          {(balance) => <AssetCard balance={balance()} />}
        </For>
      </div>
      <div class="actions">
        <Link href="/assets/add" class="button primary" testid="add-asset">
          + Add an asset
        </Link>
      </div>
      <Show when={data.rates().some((rate) => rate.assetCode !== "USD")}>
        <Card title="Exchange rates" testid="rates">
          <ul class="rate-list">
            <For
              each={data.rates().filter((rate) => rate.assetCode !== "USD")}
            >
              {(rate) => (
                <li
                  data-testid={`rate-${rate.assetCode}`}
                  data-stale={rate.stale ? "yes" : "no"}
                >
                  <span class="rate">
                    {rateText(rate.assetCode, rate.perUsd)}
                  </span>{" "}
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

function AssetCard(props: { balance: AssetBalance }): JSX.Element {
  const data = useNodeData();
  const name = () => props.balance.name;
  const currency = () => ledgerCurrency(props.balance);
  // How each of its ledgers stands, once each, while one is not synced.
  const connection = () => [
    ...new Set(
      props.balance.ledgers.flatMap((id) => {
        const ledger = data.ledgers()?.find((entry) => entry.id === id);
        const text = ledger && connectionText(ledger);
        return text ? [text] : [];
      }),
    ),
  ];
  return (
    <Card testid={`asset-${name()}`} class="asset-card">
      <div class="ledger-head">
        <h2>{assetTitle(props.balance)}</h2>
        <Link
          href={`/assets/${name()}`}
          class="button"
          testid={`asset-open-${name()}`}
          aria-label={`Open ${assetTitle(props.balance)}`}
        >
          Open
        </Link>
      </div>
      <div class="ledger-balance">
        <ValueCube />
        <Amount
          value={props.balance.ownerBalance}
          currency={currency()}
          size="lg"
          testid={`asset-balance-${name()}`}
        />
      </div>
      <Show when={showsSpendable(props.balance)}>
        <p class="muted" data-testid={`asset-spendable-${name()}`}>
          <Amount value={props.balance.spendable} currency={currency()} />{" "}
          spendable
        </p>
      </Show>
      <For each={connection()}>
        {(text) => <p data-testid={`asset-connection-${name()}`}>{text}</p>}
      </For>
    </Card>
  );
}
