/**
 * Depositing on a ledger that settles in a token (spec/areas/wallet.md#pages),
 * such as RLUSD or DSUSD on the XRP Ledger testnet: the account needs both the
 * token and the ledger's own asset, which pays its fees and its reserve as
 * gas, at the same address. The reserve is shown in that asset, as the
 * ledger holds it, never in the token
 * (spec/decisions/external-assets.md).
 */

import type { JSX } from "@solidjs/web";
import {
  type ExternalAsset,
  type Ledger,
  LedgerConnection,
  Realm,
} from "@dassie/rpc/proto";
import { Show } from "solid-js";
import { Amount } from "@dassie/ui/amount";
import { Alert, Badge } from "@dassie/ui/ui";
import { freezeText, holdingCurrency, holdingOf } from "../assets/assets.ts";
import { schemeName, type TokenScheme } from "../schemes.ts";
import { DepositAddress } from "./DepositAddress.tsx";
import { Faucet } from "./Faucet.tsx";

export function TokenDeposit(props: {
  ledger: Ledger;
  address: string;
  token: TokenScheme;
  /** The account the scheme settles through, as its ledger says. */
  asset: ExternalAsset | undefined;
}): JSX.Element {
  const network = () => schemeName(props.ledger.settlementScheme);
  const testnet = () => props.ledger.realm === Realm.TEST;
  const gas = () => holdingOf(props.asset, props.token.gas);
  const line = () => holdingOf(props.asset, props.token.token);
  const id = () => props.ledger.id;
  const frozen = () => {
    const held = line();
    return held && freezeText(held);
  };
  return (
    <div class="stack">
      <dl class="facts">
        <dt>Network</dt>
        <dd>
          {network()}{" "}
          <Show when={testnet()}>
            <Badge testid={`deposit-testnet-${id()}`}>Testnet</Badge>
          </Show>
        </dd>
      </dl>
      <Show when={props.ledger.connection === LedgerConnection.UNFUNDED}>
        <Alert
          title="Not activated yet"
          tone="warning"
          testid={`deposit-unfunded-${id()}`}
        >
          The account does not exist on the ledger yet. Send it{" "}
          {props.token.gas} first, from the testnet faucet or another account:
          {" "}
          {props.token.token} cannot arrive before it exists.
        </Alert>
      </Show>
      <DepositAddress
        ledger={props.ledger}
        address={props.address}
        testid="deposit"
        pixels={200}
      />
      <dl class="facts">
        <dt>{props.token.token}</dt>
        <dd data-testid={`deposit-token-${id()}`}>
          Send {props.token.token} of {props.token.issuerName}'s issuer
          <Show when={props.ledger.issuer}>
            {(issuer) => (
              <>
                ,{" "}
                <code data-testid={`deposit-issuer-${id()}`}>{issuer()}</code>,
              </>
            )}
          </Show>{" "}
          to this address. It arrives on the account's trust line to the issuer,
          which the node sets up itself.
          <Show when={props.token.obtain}>
            {(obtain) => (
              <span data-testid={`deposit-obtain-${id()}`}>
                {" "}
                {obtain()}
              </span>
            )}
          </Show>
          <Show
            when={line()}
            fallback={` The ledger does not show the line yet: until it does, no ${props.token.token} can arrive.`}
          >
            {(held) => (
              <>
                {" "}It holds{" "}
                <Amount
                  value={held().amount}
                  currency={holdingCurrency(held())}
                  testid={`deposit-token-held-${id()}`}
                />.
              </>
            )}
          </Show>
        </dd>
        <dt>{props.token.gas} for fees</dt>
        <dd data-testid={`deposit-gas-${id()}`}>
          The account pays its fees and its trust line's reserve in{" "}
          {props.token.gas}, which is not in this ledger's balance. Send{" "}
          {props.token.gas}{" "}
          to the same address, or get it from the testnet faucet below.
          <Show when={gas()}>
            {(held) => (
              <>
                {" "}It holds{" "}
                <Amount
                  value={held().amount}
                  currency={holdingCurrency(held())}
                  testid={`deposit-gas-held-${id()}`}
                />, of which the ledger holds back{" "}
                <Amount
                  value={held().reserve}
                  currency={holdingCurrency(held())}
                  testid={`deposit-reserve-${id()}`}
                />{" "}
                as a reserve: never spent.
              </>
            )}
          </Show>
        </dd>
        <dt>Destination tag</dt>
        <dd data-testid={`deposit-tag-${id()}`}>
          Not needed: the address is this node's alone. Leave the tag empty; a
          payment with one still arrives.
        </dd>
        <dt>Arrives</dt>
        <dd>Once the ledger validates the payment, usually within seconds.</dd>
      </dl>
      <Show when={frozen()}>
        {(text) => (
          <Alert
            title="Frozen by its issuer"
            tone="danger"
            testid={`deposit-frozen-${id()}`}
          >
            {text()}
          </Alert>
        )}
      </Show>
      <Alert
        title={`Send only ${props.token.token} and ${props.token.gas} on the ${network()}`}
        tone="warning"
        testid={`deposit-warning-${id()}`}
      >
        Other tokens, {props.token.token}{" "}
        of another issuer, or anything sent on another network, may be lost.
        <Show when={testnet()}>
          {" "}Test {props.token.token} and test {props.token.gas}{" "}
          have no value. Never send real ones here: the node does not follow the
          live ledger.
        </Show>
      </Alert>
      <Show when={props.ledger.faucet}>
        <Faucet
          scheme={props.ledger.settlementScheme}
          assetCode={props.token.gas}
          purpose="for fees"
        />
      </Show>
    </div>
  );
}
