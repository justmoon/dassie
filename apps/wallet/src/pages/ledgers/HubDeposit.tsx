/**
 * Depositing into the node's account in a settlement hub
 * (spec/areas/wallet.md#pages), while the owner keeps what the node
 * receives in the hub: the hub's escrow on the XRP Ledger as a QR code and
 * text, and the destination tag that names the account, which every
 * deposit must carry (spec/areas/hubs.md#the-escrow). Laid out as
 * exchanges lay out an address with a tag.
 */

import type { JSX } from "@solidjs/web";
import { type Ledger, Realm } from "@dassie/rpc/proto";
import { Show } from "solid-js";
import { Alert, Badge } from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { DepositAddress } from "./DepositAddress.tsx";

export function HubDeposit(
  props: { ledger: Ledger; address: string; tag: number; asset: string },
): JSX.Element {
  const network = () => schemeName(props.ledger.settlementScheme);
  const testnet = () => props.ledger.realm === Realm.TEST;
  const id = () => props.ledger.id;
  return (
    <div class="stack">
      <p>
        You keep what you receive in {props.asset}{" "}
        in your settlement hub, so a deposit goes there too: to the hub's escrow
        on the XRP Ledger, with your account's destination tag.
      </p>
      <dl class="facts">
        <dt>Network</dt>
        <dd>
          {network()}{" "}
          <Show when={testnet()}>
            <Badge testid={`deposit-testnet-${id()}`}>Testnet</Badge>
          </Show>
        </dd>
      </dl>
      <DepositAddress
        ledger={props.ledger}
        address={props.address}
        testid="deposit"
        pixels={200}
      />
      <dl class="facts">
        <dt>Arrives</dt>
        <dd>
          Once the hub's validators see the payment validated, usually within
          seconds.
        </dd>
      </dl>
      <Alert
        title={`Always send destination tag ${props.tag}`}
        tone="warning"
        testid={`deposit-tag-warning-${id()}`}
      >
        The tag names your account in the hub. The escrow refuses a payment
        without one, and a payment with another tag is credited to someone
        else's account. Send only {props.asset}
        <Show when={props.asset !== "XRP"}>, from its issuer</Show>.
        <Show when={testnet()}>
          {" "}Test {props.asset} has no value. Never send real {props.asset}
          {" "}
          here.
        </Show>
      </Alert>
      <p class="muted">
        To keep it on your own account instead, change where you keep{" "}
        {props.asset} in Settings.
      </p>
    </div>
  );
}
