/**
 * Paired devices (spec/areas/wallet.md#pages): the owner's apps that call the
 * node with a device key, each revoked on its own, and the pairing code for a
 * new one (spec/areas/rpc.md#paired-devices). The node starts a pairing only
 * for a browser on its own machine; elsewhere the card says so.
 */

import type { JSX } from "@solidjs/web";
import type { StartPairingResponse } from "@dassie/rpc/proto";
import { createMemo, createSignal, For, Show } from "solid-js";
import { dateTime } from "@dassie/ui/format";
import { Qr } from "@dassie/ui/qr";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import { Card, ConfirmButton, CopyButton, ErrorText } from "@dassie/ui/ui";

export function Devices(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const [version, setVersion] = createSignal(0);
  const [pairing, setPairing] = createSignal<StartPairingResponse>();
  const [error, setError] = createSignal<string>();
  const devices = createMemo(async () => {
    version();
    return (await client.call("listDevices", {})).devices;
  });
  const pair = async () => {
    setError(undefined);
    try {
      setPairing(await client.call("startPairing", {}));
    } catch (failure) {
      setPairing(undefined);
      setError(describe(failure));
    }
  };
  const revoke = async (id: string) => {
    try {
      await client.call("revokeDevice", { id });
      toast({ title: "Device revoked" });
      setVersion((n) => n + 1);
    } catch (failure) {
      toast({
        title: "That did not work",
        description: describe(failure),
        tone: "destructive",
      });
    }
  };
  return (
    <Card title="Paired devices" testid="devices">
      <p class="muted">Apps that reach your node from your other devices</p>
      <ul class="sessions">
        <For each={devices()}>
          {(device) => (
            <li data-testid={`device-${device.id}`}>
              <div>{device.name}</div>
              <div class="muted">
                paired {dateTime(device.pairedAtUnixMs)}, last seen{" "}
                {dateTime(device.lastSeenAtUnixMs)}
              </div>
              <ConfirmButton
                label="Revoke"
                title="Revoke this device?"
                confirm="Revoke it"
                tone="danger"
                testid={`revoke-device-${device.id}`}
                onConfirm={() => revoke(device.id)}
              >
                <p>The app loses access at once, and must be paired again.</p>
              </ConfirmButton>
            </li>
          )}
        </For>
      </ul>
      <Show when={devices()?.length === 0}>
        <p class="muted" data-testid="devices-none">No devices yet.</p>
      </Show>
      <Show when={pairing()}>
        {(started) => (
          <div class="deposit pairing" data-testid="pairing">
            <Qr
              text={started().code}
              label="Pairing code"
              testid="pairing-qr"
            />
            <div>
              <p>
                Scan this code in the app, or paste it there. It works once,
                until {dateTime(started().expiresAtUnixMs)}.
              </p>
              <CopyButton text={started().code} label="Copy code" />
            </div>
          </div>
        )}
      </Show>
      <ErrorText message={error()} testid="pairing-error" />
      <div class="actions">
        <button type="button" data-testid="pair-device" onClick={pair}>
          {pairing() ? "New code" : "Pair a device"}
        </button>
      </div>
    </Card>
  );
}
