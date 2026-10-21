/** Test money from a test ledger's faucet, which the node asks
 * (spec/areas/xrpl.md#deposits-and-withdrawals). */

import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import { ErrorText } from "@dassie/ui/ui";

export function Faucet(props: {
  /** A scheme on the account the faucet funds. */
  scheme: string;
  /** What the faucet gives: the ledger's own asset, such as "XRP". */
  assetCode: string;
  /** What it is for, if not the balance. */
  purpose?: string;
}): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const ask = async () => {
    setPending(true);
    setError(undefined);
    try {
      await client.call("askFaucet", { settlementScheme: props.scheme });
      toast({
        title: "Asked the testnet faucet",
        description:
          `The test ${props.assetCode} shows up once the ledger validates it.`,
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
          Get free test {props.assetCode}
          {props.purpose ? ` ${props.purpose}` : ""}{" "}
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
          Get test {props.assetCode}
        </button>
      </div>
      <ErrorText message={error()} />
    </section>
  );
}
