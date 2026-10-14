/**
 * The node's wallet addresses (D9): one per ledger, with the default one
 * at `$host`, and a way to choose which ledger that is.
 */

import type { JSX } from "@solidjs/web";
import type { WalletAddress } from "@dassie/rpc/proto";
import { createSignal, For, Show } from "solid-js";
import { describe, useClient } from "@dassie/ui/rpc";
import { Badge, CopyButton, ErrorText } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";

/** Loads the wallet addresses once; `reload` asks again. */
export function useWalletAddresses(): {
  addresses: () => readonly WalletAddress[] | undefined;
  error: () => string | undefined;
  reload: () => Promise<void>;
  set: (addresses: readonly WalletAddress[]) => void;
} {
  const client = useClient();
  const [addresses, setAddresses] = createSignal<readonly WalletAddress[]>();
  const [error, setError] = createSignal<string>();
  const reload = async () => {
    try {
      const response = await client.call("listWalletAddresses", {});
      setAddresses(response.walletAddresses);
      setError(undefined);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  void reload();
  return { addresses, error, reload, set: setAddresses };
}

/** The ledger's name in the wallet: its scheme's, from the ledger ID. */
function ledgerName(ledger: string): string {
  return schemeName(ledger.split("+")[0]);
}

export function WalletAddresses(): JSX.Element {
  const client = useClient();
  const state = useWalletAddresses();
  const [error, setError] = createSignal<string>();
  const makeDefault = async (ledger: string) => {
    setError(undefined);
    try {
      const response = await client.call("setDefaultLedger", { ledger });
      state.set(response.walletAddresses);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <div class="stack" data-testid="wallet-addresses">
      <p>
        Other wallets pay you at these addresses, through Open Payments. A
        payment pointer is the same address, shorter.
      </p>
      <ErrorText message={state.error() ?? error()} />
      <ul class="addresses">
        <For each={state.addresses() ?? []} keyed={(entry) => entry.ledger}>
          {(address) => (
            <li data-testid={`wallet-address-${address().ledger}`}>
              <div class="address-head">
                <strong>{ledgerName(address().ledger)}</strong>
                <span class="muted">{address().assetCode}</span>
                <Show when={address().isDefault}>
                  <Badge tone="good" testid="default-address">default</Badge>
                </Show>
              </div>
              <p class="pointer boxed">
                <span data-testid="receive-pointer">
                  {address().paymentPointer}
                </span>{" "}
                <CopyButton text={address().paymentPointer} />
              </p>
              <p class="muted">
                <code>{address().url}</code> <CopyButton text={address().url} />
              </p>
              <Show when={!address().isDefault}>
                <button
                  type="button"
                  data-testid={`make-default-${address().ledger}`}
                  onClick={() => makeDefault(address().ledger)}
                >
                  Make {address().paymentPointer.split("/")[0]} this ledger
                </button>
              </Show>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}
