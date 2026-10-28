/**
 * The node's wallet addresses (spec/areas/payments.md#wallet-addresses):
 * one per asset, each with the ledger what arrives is booked on, the
 * default one at `$host`, and a way to choose which asset that is.
 */

import type { JSX } from "@solidjs/web";
import type { WalletAddress } from "@dassie/rpc/proto";
import { createSignal, For, Show } from "solid-js";
import { describe, useClient } from "@dassie/ui/rpc";
import { Alert, Badge, CopyButton, ErrorText } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { useStatus } from "../../status.ts";
import { atFrontDoor, frontDoorState, type Relay } from "./front_door.ts";

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

/**
 * Says that a pointer at the default front door reaches the node through
 * it, and warns while it does not (spec/areas/payments.md#the-nodes-names).
 */
export function FrontDoorNote(props: { pointer?: string }): JSX.Element {
  return (
    <Show when={props.pointer !== undefined && atFrontDoor(props.pointer)}>
      <FrontDoorStatus pointer={props.pointer} />
    </Show>
  );
}

const NAMED =
  "Your node has no hostname, so it is named at the dassiepay.net front door, which reaches it only while your node is connected to it.";

function FrontDoorStatus(props: { pointer?: string }): JSX.Element {
  const relay = useRelay();
  const state = () => frontDoorState(props.pointer, relay());
  const warning = () => {
    switch (state()) {
      case "disconnected":
        return "It is not connected now, so payments to this address fail until it is. Check that your node can reach relay.dassiepay.net over UDP, or set a hostname.";
      case "other-relay":
        return "Your node keeps another relay, so the front door cannot reach it: don't hand this address out, or set a hostname.";
      case "no-relay":
        return "Your node keeps no relay (DASSIE_RELAY names none), so the front door cannot reach it: don't hand this address out, or set a hostname.";
      default:
        return undefined;
    }
  };
  return (
    <Show
      when={warning()}
      fallback={
        <p class="muted" data-testid="front-door-note">
          {NAMED}
          {state() === "reachable" ? " It is connected now." : ""}
          {state() === "connecting" ? " It is connecting now." : ""}
        </p>
      }
    >
      {(text) => (
        <Alert
          title="This address may reach no one"
          tone="warning"
          testid="front-door-note"
        >
          <p>{NAMED} {text()}</p>
        </Alert>
      )}
    </Show>
  );
}

/**
 * The relay the node reports, asked again every little while: `null` when
 * it keeps none, `undefined` until known.
 */
function useRelay(): () => Relay | null | undefined {
  const status = useStatus();
  return () => {
    const known = status();
    return known === undefined ? undefined : (known.relay ?? null);
  };
}

/** The ledger's name in the wallet: its scheme's, from the ledger ID. */
export function ledgerName(ledger: string): string {
  return schemeName(ledger.split("+")[0]);
}

export function WalletAddresses(): JSX.Element {
  const client = useClient();
  const state = useWalletAddresses();
  const [error, setError] = createSignal<string>();
  const makeDefault = async (name: string) => {
    setError(undefined);
    try {
      const response = await client.call("setDefaultWalletAddress", { name });
      state.set(response.walletAddresses);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <div class="stack" data-testid="wallet-addresses">
      <p>
        Other wallets pay you at these addresses, through Open Payments: one per
        asset. A payment pointer is the same address, shorter.
      </p>
      <ErrorText message={state.error() ?? error()} />
      <FrontDoorNote pointer={state.addresses()?.[0]?.paymentPointer} />
      <ul class="addresses">
        <For each={state.addresses() ?? []} keyed={(entry) => entry.name}>
          {(address) => (
            <li data-testid={`wallet-address-${address().name}`}>
              <div class="address-head">
                <strong>{assetLabel(address().name)}</strong>
                <span class="muted">
                  arrives in {ledgerName(address().ledger)}
                </span>
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
                  data-testid={`make-default-${address().name}`}
                  onClick={() => makeDefault(address().name)}
                >
                  Make {address().paymentPointer.split("/")[0]} pay in{" "}
                  {assetLabel(address().name)}
                </button>
              </Show>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}

/** A wallet address's asset for people, from its name: `xrp` is XRP. */
export function assetLabel(name: string): string {
  return name.toUpperCase();
}
