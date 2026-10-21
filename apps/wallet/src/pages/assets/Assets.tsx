/**
 * External assets (spec/areas/wallet.md#pages): what the node holds outside
 * its books, such as its account on the XRP Ledger testnet, each with its
 * address, what it holds as its ledger says (never as the books add it up,
 * spec/decisions/external-assets.md), the reserve, and a line its issuer
 * froze. It is where the owner funds them.
 */

import type { JSX } from "@solidjs/web";
import type { ExternalAsset, Holding } from "@dassie/rpc/proto";
import { For, Show } from "solid-js";
import { Amount } from "@dassie/ui/amount";
import { useNodeData } from "@dassie/ui/node-data";
import { Qr } from "@dassie/ui/qr";
import { Link } from "@dassie/ui/router";
import { Alert, Card, CopyButton, LiveNote, PageTitle } from "@dassie/ui/ui";
import { connectionText } from "../ledgers/connection.ts";
import { Faucet } from "../ledgers/Faucet.tsx";
import { schemeName } from "../schemes.ts";
import {
  assetName,
  freezeText,
  holdingCurrency,
  nativeAsset,
} from "./assets.ts";

export function Assets(): JSX.Element {
  const data = useNodeData();
  return (
    <div class="page-wide stack">
      <PageTitle>External Assets</PageTitle>
      <p class="muted">
        What your node holds outside its books, as the ledger reports it. Your
        ledgers settle through these.
      </p>
      <LiveNote status={data.ledgersStatus()} />
      <Show
        when={data.externalAssets().length > 0}
        fallback={
          <p class="muted" data-testid="assets-none">
            None of your ledgers settles through an external asset.
          </p>
        }
      >
        <For each={data.externalAssets()} keyed={(asset) => asset.id}>
          {(asset) => <AssetCard asset={asset()} />}
        </For>
      </Show>
    </div>
  );
}

function AssetCard(props: { asset: ExternalAsset }): JSX.Element {
  const id = () => props.asset.id;
  return (
    <Card testid={`asset-${id()}`} class="ledger">
      <h2>{assetName(id())}</h2>
      <Show when={connectionText(props.asset)}>
        {(text) => <p data-testid={`asset-connection-${id()}`}>{text()}</p>}
      </Show>
      <Show when={props.asset.address}>
        {(address) => (
          <div class="deposit">
            <Qr
              text={address()}
              label={`Address ${address()}`}
              testid={`asset-qr-${id()}`}
            />
            <div>
              <p>Address</p>
              <p>
                <code data-testid={`asset-address-${id()}`}>{address()}</code>
                {" "}
                <CopyButton text={address()} label="Copy address" />
              </p>
            </div>
          </div>
        )}
      </Show>
      <Show
        when={props.asset.holdings.length > 0}
        fallback={
          <p class="muted">The ledger has not said what it holds yet.</p>
        }
      >
        <dl class="facts">
          <For each={props.asset.holdings}>
            {(holding) => <HoldingFacts asset={id()} holding={holding} />}
          </For>
        </dl>
      </Show>
      <p class="muted">
        Settles{" "}
        <For each={props.asset.settlementSchemes}>
          {(scheme, index) => (
            <>
              {index() > 0 ? " and " : ""}
              <Link
                href={`/receive?tab=${scheme}`}
                testid={`asset-scheme-${scheme}`}
              >
                {schemeName(scheme)}
              </Link>
            </>
          )}
        </For>
        .
      </p>
      <Show when={props.asset.faucet && props.asset.settlementSchemes[0]}>
        {(scheme) => (
          <Faucet scheme={scheme()} assetCode={nativeAsset(id()) ?? ""} />
        )}
      </Show>
    </Card>
  );
}

function HoldingFacts(
  props: { asset: string; holding: Holding },
): JSX.Element {
  const testid = () =>
    `asset-holding-${props.asset}-${props.holding.assetCode}`;
  const currency = () => holdingCurrency(props.holding);
  return (
    <>
      <dt>{props.holding.assetCode}</dt>
      <dd>
        <Amount
          value={props.holding.amount}
          currency={currency()}
          size="lg"
          testid={testid()}
        />
        <Show when={props.holding.issuer}>
          {(issuer) => (
            <p class="muted">
              Issued by <code>{issuer()}</code>, on the account's trust line
            </p>
          )}
        </Show>
        <Show when={props.holding.reserve > 0n}>
          <p class="muted">
            The ledger holds back{" "}
            <Amount
              value={props.holding.reserve}
              currency={currency()}
              testid={`${testid()}-reserve`}
            />{" "}
            as a reserve: never spent.
          </p>
        </Show>
        <Show when={freezeText(props.holding)}>
          {(text) => (
            <Alert
              title="Frozen by its issuer"
              tone="danger"
              testid={`${testid()}-frozen`}
            >
              {text()}
            </Alert>
          )}
        </Show>
      </dd>
    </>
  );
}
