/**
 * What an external asset holds, such as the owner's account on the XRP
 * Ledger, as its ledger says (spec/areas/wallet.md#pages): each holding
 * with the reserve and a line its issuer froze, never as the books add it
 * up (spec/decisions/external-assets.md).
 */

import type { JSX } from "@solidjs/web";
import type { ExternalAsset, Holding } from "@dassie/rpc/proto";
import { For, Show } from "solid-js";
import { Amount } from "@dassie/ui/amount";
import { Alert } from "@dassie/ui/ui";
import { freezeText, holdingCurrency } from "./assets.ts";

export function Holdings(props: { asset: ExternalAsset }): JSX.Element {
  return (
    <Show
      when={props.asset.holdings.length > 0}
      fallback={<p class="muted">The ledger has not said what it holds yet.</p>}
    >
      <dl class="facts" data-testid={`asset-holdings-${props.asset.id}`}>
        <For each={props.asset.holdings}>
          {(holding) => (
            <HoldingFacts asset={props.asset.id} holding={holding} />
          )}
        </For>
      </dl>
    </Show>
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
            as a reserve: not spendable while the account is open.
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
