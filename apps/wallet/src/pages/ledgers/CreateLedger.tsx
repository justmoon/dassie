/**
 * Choosing a ledger (spec/areas/wallet.md#the-gate): the schemes
 * the node can run, and the owner's first choice, which the wallet insists on
 * (UI-3: the list comes from the node, not a fixed stub).
 */

import type { JSX } from "@solidjs/web";
import { Realm, type SettlementSchemeInfo } from "@dassie/rpc/proto";
import { createMemo, createSignal, For, Show } from "solid-js";
import { submitLogout } from "../../auth.ts";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { useRouter } from "@dassie/ui/router";
import { Alert, Card, ErrorText, PageTitle, Spinner } from "@dassie/ui/ui";
import { schemeDescription, schemeName } from "../schemes.ts";

export function CreateLedger(): JSX.Element {
  return (
    <div class="page-narrow">
      <PageTitle>Choose a Ledger Type</PageTitle>
      <Choices />
    </div>
  );
}

/** The first ledger, before anything else: no navigation, only a way out. */
export function Onboarding(): JSX.Element {
  const data = useNodeData();
  return (
    <main id="content" class="page-narrow onboarding">
      <p class="brand">Dassie Wallet</p>
      <PageTitle>Choose a Ledger Type</PageTitle>
      <p class="muted">
        Signed in to node <code data-testid="node-id">{data.selfId}</code>
      </p>
      <Choices />
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

function Choices(): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const router = useRouter();
  const [busy, setBusy] = createSignal<string>();
  const [error, setError] = createSignal<string>();
  const schemes = createMemo(async () =>
    (await client.call("listSettlementSchemes", {})).schemes
  );
  const select = async (scheme: SettlementSchemeInfo) => {
    setBusy(scheme.id);
    setError(undefined);
    try {
      const added = await client.call("addSettlementScheme", {
        id: scheme.id,
        config: "",
      });
      if (added.ledger) data.applyLedger(added.ledger);
      router.navigate("/ledgers");
    } catch (failure) {
      setError(describe(failure));
    }
    setBusy(undefined);
  };
  return (
    <Card testid="page-create-ledger">
      <Show when={data.allocationScheme === "test"}>
        <Alert title="Testnet" testid="testnet-alert">
          You are connected to the Dassie test network. The settlement methods
          below do <strong>not</strong> use real money.
        </Alert>
      </Show>
      <ul class="choices">
        <For each={schemes()}>
          {(scheme) => (
            <li class="choice">
              <div>
                <h2>{schemeName(scheme.id, scheme.name)} Settlement</h2>
                <p class="muted">
                  {schemeDescription(scheme.id) ??
                    `Settles in ${scheme.assetCode} on ${scheme.ledger}.`}
                </p>
              </div>
              <Show
                when={!scheme.active}
                fallback={<span class="badge badge-good">Active</span>}
              >
                <button
                  type="button"
                  class="primary"
                  data-testid={`select-scheme-${scheme.id}`}
                  disabled={busy() !== undefined || scheme.realm !== Realm.TEST}
                  onClick={() => select(scheme)}
                >
                  Select →
                </button>
              </Show>
            </li>
          )}
        </For>
      </ul>
      <Show when={busy()}>
        <Spinner label="Activating…" />
      </Show>
      <ErrorText message={error()} testid="create-ledger-error" />
    </Card>
  );
}
