/**
 * Depositing on a ledger the owner funds from outside, laid out as exchanges
 * and wallets lay out theirs (spec/areas/wallet.md#pages): the network, the
 * address as a QR code and text, whether a destination tag is needed, the
 * reserve, what to send and what not, and the testnet faucet where the node
 * has one.
 */

import type { JSX } from "@solidjs/web";
import { type Ledger, LedgerConnection } from "@dassie/rpc/proto";
import { createSignal, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import { Alert, Badge, ErrorText } from "@dassie/ui/ui";
import { isTestnet, schemeName } from "../schemes.ts";
import { DepositAddress } from "./DepositAddress.tsx";

export function LedgerDeposit(
  props: { ledger: Ledger; address: string },
): JSX.Element {
  const network = () => schemeName(props.ledger.settlementScheme);
  const testnet = () => isTestnet(props.ledger.settlementScheme);
  const reserve = () => (
    <Amount
      value={props.ledger.reserve}
      currency={ledgerCurrency(props.ledger)}
    />
  );
  return (
    <div class="stack">
      <dl class="facts">
        <dt>Network</dt>
        <dd>
          {network()}{" "}
          <Show when={testnet()}>
            <Badge>Testnet</Badge>
          </Show>
        </dd>
      </dl>
      <Show when={props.ledger.connection === LedgerConnection.UNFUNDED}>
        <Alert
          title="Not activated yet"
          tone="warning"
          testid={`deposit-unfunded-${props.ledger.id}`}
        >
          The account does not exist on the ledger yet.{" "}
          <Show
            when={props.ledger.reserve > 0n}
            fallback={`Send ${props.ledger.assetCode} to activate it.`}
          >
            Send at least {reserve()} to activate it.
          </Show>
          <Show when={props.ledger.faucet}>
            {" "}The node also asks the testnet faucet until it exists.
          </Show>
        </Alert>
      </Show>
      <DepositAddress
        ledger={props.ledger}
        address={props.address}
        testid="deposit"
        pixels={200}
      />
      <dl class="facts">
        <dt>Destination tag</dt>
        <dd data-testid={`deposit-tag-${props.ledger.id}`}>
          Not needed: the address is this node's alone. Leave the tag empty; a
          payment with one still arrives.
        </dd>
        <Show when={props.ledger.reserve > 0n}>
          <dt>Reserve</dt>
          <dd>
            The ledger holds back {reserve()}{" "}
            on the account: never spent, and not in your balance.
          </dd>
        </Show>
        <dt>Arrives</dt>
        <dd>Once the ledger validates the payment, usually within seconds.</dd>
      </dl>
      <Alert
        title={`Send only ${props.ledger.assetCode} on the ${network()}`}
        tone="warning"
        testid={`deposit-warning-${props.ledger.id}`}
      >
        Other assets, or {props.ledger.assetCode}{" "}
        sent on another network, may be lost.
        <Show when={testnet()}>
          {" "}Test {props.ledger.assetCode} has no value. Never send real{" "}
          {props.ledger.assetCode}{" "}
          here: the node does not follow the live ledger.
        </Show>
      </Alert>
      <Show when={props.ledger.faucet}>
        <Faucet ledger={props.ledger} />
      </Show>
    </div>
  );
}

/** Test money from the ledger's faucet, which the node asks. */
function Faucet(props: { ledger: Ledger }): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const ask = async () => {
    setPending(true);
    setError(undefined);
    try {
      await client.call("askFaucet", {
        settlementScheme: props.ledger.settlementScheme,
      });
      toast({
        title: "Asked the testnet faucet",
        description:
          `The test ${props.ledger.assetCode} shows up in your balance once the ledger validates it.`,
      });
    } catch (failure) {
      setError(describe(failure));
    }
    setPending(false);
  };
  return (
    <section class="stack">
      <div>
        <h3>Testnet faucet</h3>
        <p class="muted">
          Get free test {props.ledger.assetCode}{" "}
          from the testnet faucet, once a minute at most.
        </p>
      </div>
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-testid="faucet"
          disabled={pending()}
          onClick={ask}
        >
          Get test {props.ledger.assetCode}
        </button>
      </div>
      <ErrorText message={error()} />
    </section>
  );
}
