/**
 * Choosing assets (spec/areas/wallet.md#the-gate): a checklist of the
 * assets the node offers and does not hold yet, XRP ticked to start with,
 * and one Continue. The owner chooses assets, never schemes: the node adds
 * each asset's account and its hub, and first what it needs
 * (spec/decisions/owner-chooses-assets.md). Without a ledger it is all
 * the wallet shows; later it opens from Assets.
 */

import type { JSX } from "@solidjs/web";
import type { AssetInfo } from "@dassie/rpc/proto";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { submitLogout } from "../../auth.ts";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Link, useRouter } from "@dassie/ui/router";
import { Alert, Card, ErrorText, PageTitle, Spinner } from "@dassie/ui/ui";
import { useToast } from "@dassie/ui/toast";
import { Lockup } from "@dassie/ui/kopje";
import { assetText } from "../schemes.ts";
import { assetLabel } from "../payments/WalletAddresses.tsx";
import { choice, choosable, initialChoice, offered, toggle } from "./choose.ts";

/** Adding assets from Assets, in the wallet. */
export function AddAssets(): JSX.Element {
  return (
    <div class="page-narrow">
      <p class="breadcrumb">
        <Link href="/assets">Assets</Link> ›
      </p>
      <PageTitle>Add an asset</PageTitle>
      <Choices done="/assets" />
    </div>
  );
}

/** The first assets, before anything else: no navigation, only a way out. */
export function Onboarding(): JSX.Element {
  const data = useNodeData();
  return (
    <main id="content" class="page-narrow onboarding">
      <p class="brand">
        <Lockup suffix="Wallet" />
      </p>
      <PageTitle>Choose what your node holds</PageTitle>
      <p class="muted">
        Signed in to node <code data-testid="node-id">{data.selfId}</code>
      </p>
      <Choices done="/" />
      <div class="actions">
        <button
          type="button"
          data-testid="logout"
          onClick={async () => {
            await submitLogout();
            location.reload();
          }}
        >
          Log out
        </button>
      </div>
    </main>
  );
}

function Choices(props: { done: string }): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const router = useRouter();
  const toast = useToast();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const rows = createMemo(async () =>
    offered((await client.call("listAssets", {})).assets)
  );
  const [chosen, setChosen] = createSignal<readonly string[]>([]);
  // XRP starts ticked, once the rows are known.
  createEffect(rows, (known) => {
    setChosen(initialChoice(known));
  });
  const state = () => choice(rows(), chosen());
  const add = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const added = await client.call("addAssets", { names: state().ticked });
      for (const ledger of added.ledgers) data.applyLedger(ledger);
      router.navigate(props.done);
    } catch (failure) {
      setError(describe(failure));
      setBusy(false);
      // What was added before the failure stays, and its ledgers may open
      // the wallet in place of this page, error and all: a notice outlives
      // the switch.
      toast({
        title: "Not every asset was added",
        description: `${describe(failure)} Add it again on Assets.`,
        tone: "destructive",
      });
    }
  };
  return (
    <Card testid="page-choose-assets">
      <Show when={data.allocationScheme === "test"}>
        <Alert title="Testnet" testid="testnet-alert">
          You are connected to the Dassie test network. Nothing below is real
          money.
        </Alert>
      </Show>
      <Show
        when={rows().length > 0}
        fallback={
          <p class="muted" data-testid="choose-assets-none">
            Your node holds every asset it offers.
          </p>
        }
      >
        <ul class="choices">
          <For each={rows()} keyed={(asset) => asset.name}>
            {(asset) => (
              <AssetRow
                asset={asset()}
                ticked={state().ticked.includes(asset().name)}
                neededBy={state().neededBy[asset().name]}
                disabled={busy()}
                onToggle={() => setChosen(toggle(chosen(), asset().name))}
              />
            )}
          </For>
        </ul>
        <div class="actions">
          <button
            type="button"
            class="primary"
            data-testid="assets-continue"
            disabled={busy() || state().ticked.length === 0}
            onClick={add}
          >
            Continue →
          </button>
        </div>
      </Show>
      <Show when={busy()}>
        <Spinner label="Adding…" />
      </Show>
      <ErrorText message={error()} testid="choose-assets-error" />
    </Card>
  );
}

function AssetRow(props: {
  asset: AssetInfo;
  ticked: boolean;
  /** The ticked assets that need this one, which lock it. */
  neededBy: readonly string[] | undefined;
  disabled: boolean;
  onToggle: () => void;
}): JSX.Element {
  const text = () => assetText(props.asset.name, props.asset.realm);
  const locked = () => props.neededBy !== undefined;
  return (
    <li class="choice choice-check">
      <label>
        <input
          type="checkbox"
          data-testid={`choose-asset-${props.asset.name}`}
          checked={props.ticked}
          disabled={props.disabled || locked() || !choosable(props.asset)}
          onChange={props.onToggle}
        />
        <span class="asset-mark" aria-hidden="true">
          {props.asset.assetCode}
        </span>
        <div>
          <h2>{text()?.label ?? assetLabel(props.asset.name)}</h2>
          <p class="muted">
            {text()?.description ??
              `${props.asset.assetCode}, settled through ${
                props.asset.schemes.join(" and ")
              }.`}
          </p>
          <Show when={props.neededBy}>
            {(by) => (
              <p
                class="muted choice-note"
                data-testid={`choose-asset-needed-${props.asset.name}`}
              >
                Needed for {by().map(assetLabel).join(" and ")}'s fees.
              </p>
            )}
          </Show>
        </div>
      </label>
    </li>
  );
}
