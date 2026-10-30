/**
 * An asset's own page (spec/areas/wallet.md#pages): its one balance
 * (spec/decisions/one-balance-per-asset.md) by place, with what each part
 * sets aside, ending on what a payment may spend now; Send and Receive in
 * it; its deposit method; what its account on the ledger holds, as the
 * ledger says (spec/decisions/external-assets.md); its settlement hub,
 * whose details are on Hubs; and its ledgers, each to remove.
 */

import type { JSX } from "@solidjs/web";
import type { AssetBalance, Ledger } from "@dassie/rpc/proto";
import { For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Link, type RouteProps } from "@dassie/ui/router";
import { Alert, Card, LiveNote, PageTitle, ValueCube } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { connectionText } from "../ledgers/connection.ts";
import { Deposit } from "../ledgers/Deposit.tsx";
import { PegGuard } from "../ledgers/PegGuard.tsx";
import { removingNotice } from "../ledgers/removal.ts";
import { ReceiveIcon, SendIcon } from "../payments/icons.tsx";
import {
  assetLedgers,
  assetOf,
  assetTitle,
  breakdownRows,
  isHubLedger,
  type Segment,
} from "./assets.ts";
import { Holdings } from "./Holdings.tsx";

export function Asset(props: RouteProps): JSX.Element {
  const data = useNodeData();
  const balance = () =>
    data.balances().find((entry) => entry.name === props.params.name);
  return (
    <div class="page-wide">
      <p class="breadcrumb">
        <Link href="/assets" testid="asset-back">Assets</Link> ›
      </p>
      <Show
        when={balance()}
        fallback={
          <>
            <PageTitle>No such asset</PageTitle>
            <LiveNote status={data.ledgersStatus()} />
            <p class="muted" data-testid="asset-none">
              Your node holds no {props.params.name}.{" "}
              <Link href="/assets/add">Add an asset</Link>.
            </p>
          </>
        }
      >
        {(found) => <AssetPage balance={found()} />}
      </Show>
    </div>
  );
}

function AssetPage(props: { balance: AssetBalance }): JSX.Element {
  const data = useNodeData();
  const name = () => props.balance.name;
  const find = (id: string | undefined) =>
    id === undefined
      ? undefined
      : data.ledgers()?.find((ledger) => ledger.id === id);
  const book = () => find(assetLedgers(props.balance).book);
  const hub = () => find(assetLedgers(props.balance).hub);
  const ledgers = () =>
    props.balance.ledgers.flatMap((id) => {
      const ledger = find(id);
      return ledger ? [ledger] : [];
    });
  const account = () => {
    const scheme = book()?.settlementScheme;
    return scheme ? assetOf(scheme, data.externalAssets()) : undefined;
  };
  return (
    <>
      <PageTitle>{assetTitle(props.balance)}</PageTitle>
      <LiveNote status={data.ledgersStatus()} />
      <div class="asset-page">
        <div class="stack">
          <Breakdown balance={props.balance} />
          <div class="home-actions">
            <Link
              href={`/send?asset=${name()}`}
              class="button primary"
              testid="asset-send"
            >
              <SendIcon />
              Send {props.balance.assetCode}
            </Link>
            <Link
              href={`/receive?tab=${
                book()?.settlementScheme ?? hub()?.settlementScheme ?? ""
              }`}
              class="button"
              testid="asset-receive"
            >
              <ReceiveIcon />
              Receive {props.balance.assetCode}
            </Link>
          </div>
        </div>
        <div class="stack">
          <Show when={book()}>
            {(ledger) => (
              <Card title="Deposit" testid={`asset-deposit-${name()}`}>
                <Deposit id={ledger().id} />
              </Card>
            )}
          </Show>
          <Show when={account()}>
            {(external) => (
              <Card
                title="What the XRP Ledger holds"
                testid={`asset-account-${name()}`}
              >
                <p class="muted">
                  Your account{" "}
                  <code>{external().address}</code>, as the ledger reports it.
                  The same account may hold other assets of yours.
                </p>
                <Holdings asset={external()} />
              </Card>
            )}
          </Show>
          <Show when={hub()}>
            {(ledger) => (
              <Card title="Hub" testid={`asset-hub-${name()}`}>
                <p class="muted">{schemeName(ledger().settlementScheme)}</p>
                <Show when={connectionText(ledger())}>
                  {(text) => <p>{text()}</p>}
                </Show>
                <p>
                  <Link href="/hubs" testid="asset-hub-details">
                    Hub details on the Hubs page
                  </Link>
                </p>
              </Card>
            )}
          </Show>
          <Card title="Ledgers" testid={`asset-ledgers-${name()}`}>
            <ul class="asset-ledgers">
              <For each={ledgers()} keyed={(ledger) => ledger.id}>
                {(ledger) => <LedgerRow ledger={ledger()} />}
              </For>
            </ul>
          </Card>
        </div>
      </div>
    </>
  );
}

/** The balance, big, and where it is, ending on what a payment may spend
 * now. */
function Breakdown(props: { balance: AssetBalance }): JSX.Element {
  const name = () => props.balance.name;
  const currency = () => ledgerCurrency(props.balance);
  const rows = () => breakdownRows(props.balance);
  const line = (segments: readonly Segment[]) => (
    <For each={segments}>
      {(segment) =>
        typeof segment === "string"
          ? segment
          : <Amount value={segment.amount} currency={currency()} />}
    </For>
  );
  return (
    <Card class="asset-balance" testid={`asset-breakdown-${name()}`}>
      <h2 class="overline">Balance</h2>
      <div class="ledger-balance">
        <ValueCube />
        <Amount
          value={props.balance.ownerBalance}
          currency={currency()}
          size="lg"
          testid={`asset-balance-${name()}`}
        />
      </div>
      <Show when={rows().length > 0}>
        <ul class="breakdown">
          <For each={rows()} keyed={(row) => row.id}>
            {(row) => (
              <li data-testid={`asset-part-${row().id}`}>
                <span class="breakdown-name">{row().label}</span>
                <Amount value={row().amount} currency={currency()} />
                <Show when={row().sub.length > 0}>
                  <span class="breakdown-sub">{line(row().sub)}</span>
                </Show>
              </li>
            )}
          </For>
        </ul>
        <p class="breakdown-foot" data-testid="asset-spendable">
          You can pay{" "}
          <Amount value={props.balance.spendable} currency={currency()} /> now
          <Show when={props.balance.spendableHeld} fallback=".">
            {(why) => <>: only what is in the hub counts, since {why()}.</>}
          </Show>
        </p>
      </Show>
    </Card>
  );
}

/** One of the asset's ledgers: the owner book or its hub's. */
function LedgerRow(props: { ledger: Ledger }): JSX.Element {
  const scheme = () => props.ledger.settlementScheme;
  return (
    <li data-testid={`ledger-${props.ledger.id}`}>
      <div class="ledger-head">
        <h3>{schemeName(scheme())}</h3>
        <Link
          href={`/assets/remove/${scheme()}`}
          class="button"
          testid={`ledger-delete-${scheme()}`}
          aria-label={`Remove ${schemeName(scheme())}`}
        >
          Remove
        </Link>
      </div>
      <Show when={connectionText(props.ledger)}>
        {(text) => (
          <p data-testid={`ledger-connection-${props.ledger.id}`}>{text()}</p>
        )}
      </Show>
      <Show when={removingNotice(props.ledger.removingMethods)}>
        {(notice) => (
          <Alert
            title={notice().title}
            testid={`ledger-removing-${props.ledger.id}`}
          >
            {notice().text}
          </Alert>
        )}
      </Show>
      {/* The owner book's guard shows with its deposit. */}
      <Show when={isHubLedger(props.ledger.id)}>
        <PegGuard ledger={props.ledger} />
      </Show>
    </li>
  );
}
