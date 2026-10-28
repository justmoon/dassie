/**
 * Where the node keeps what it receives (spec/areas/wallet.md#pages,
 * spec/areas/payments.md#where-received-money-is-kept): for each asset a
 * settlement hub holds, in the hub, where payments arrive, or on the
 * owner's own account, which the node then withdraws to beyond a float.
 * The node keeps the choice and acts on it; the page only asks.
 */

import type { JSX } from "@solidjs/web";
import { FundsKept, type WalletAddress } from "@dassie/rpc/proto";
import { createSignal, For, Show } from "solid-js";
import { describe, useClient } from "@dassie/ui/rpc";
import { Card, ErrorText } from "@dassie/ui/ui";
import {
  assetLabel,
  useWalletAddresses,
} from "../payments/WalletAddresses.tsx";

export function Funds(): JSX.Element {
  const client = useClient();
  const state = useWalletAddresses();
  const [error, setError] = createSignal<string>();
  const kept = () =>
    (state.addresses() ?? []).filter((address) =>
      address.fundsKept !== FundsKept.UNSPECIFIED
    );
  const keep = async (address: WalletAddress, fundsKept: FundsKept) => {
    setError(undefined);
    try {
      const response = await client.call("setFundsKept", {
        name: address.name,
        fundsKept,
      });
      state.set(response.walletAddresses);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <Show when={kept().length > 0}>
      <Card title="Received Funds" testid="settings-funds">
        <p class="muted">
          Payments arrive in your settlement hub. Keep them there, ready to
          spend, or have your node move what is beyond a float to your own
          account on the ledger.
        </p>
        <For each={kept()} keyed={(address) => address.name}>
          {(address) => (
            <fieldset class="radios">
              <legend>{assetLabel(address().name)}</legend>
              <label>
                <input
                  type="radio"
                  name={`funds-${address().name}`}
                  data-testid={`funds-hub-${address().name}`}
                  checked={address().fundsKept === FundsKept.HUB}
                  onChange={() => keep(address(), FundsKept.HUB)}
                />{" "}
                In the settlement hub
              </label>
              <label>
                <input
                  type="radio"
                  name={`funds-${address().name}`}
                  data-testid={`funds-account-${address().name}`}
                  checked={address().fundsKept === FundsKept.OWN_ACCOUNT}
                  onChange={() => keep(address(), FundsKept.OWN_ACCOUNT)}
                />{" "}
                On your own account
              </label>
            </fieldset>
          )}
        </For>
        <ErrorText message={state.error() ?? error()} />
      </Card>
    </Show>
  );
}
