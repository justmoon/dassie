/**
 * Removing a ledger (spec 6 §1.6.9): what blocks it, as the node reports
 * it live, and a typed confirmation.
 */

import type { JSX } from "@solidjs/web";
import { createSignal, createUniqueId, Show } from "solid-js";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Navigate, type RouteProps, useRouter } from "@dassie/ui/router";
import { useToast } from "@dassie/ui/toast";
import {
  Alert,
  Card,
  Checklist,
  ErrorText,
  Field,
  PageTitle,
} from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import { expectedConfirmation, removalChecklist } from "./removal.ts";

export function DeleteLedger(props: RouteProps): JSX.Element {
  const data = useNodeData();
  const ledger = () =>
    data.ledgers()?.find((entry) =>
      entry.settlementScheme === props.params.scheme
    );
  return (
    <Show when={ledger()} fallback={<Navigate href="/ledgers" />}>
      {(found) => <Removal scheme={found().settlementScheme} />}
    </Show>
  );
}

function Removal(props: { scheme: string }): JSX.Element {
  const data = useNodeData();
  const client = useClient();
  const router = useRouter();
  const toast = useToast();
  const id = createUniqueId();
  const [typed, setTyped] = createSignal("");
  const [touched, setTouched] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const blockers = () =>
    data.ledgers()?.find((ledger) => ledger.settlementScheme === props.scheme)
      ?.removalBlockers ?? [];
  const confirmed = () => typed() === expectedConfirmation(props.scheme);
  const remove = async () => {
    setPending(true);
    setError(undefined);
    try {
      await client.call("removeSettlementScheme", { id: props.scheme });
      toast({ title: `Removed ${schemeName(props.scheme)}` });
      router.navigate("/ledgers");
    } catch (failure) {
      setError(describe(failure));
      setPending(false);
    }
  };
  return (
    <div class="page-narrow">
      <PageTitle>Removing Ledger {schemeName(props.scheme)}</PageTitle>
      <Card testid="page-delete-ledger">
        <Alert title="Possibility to lose funds" tone="danger">
          Removing a ledger stops your node from using it. What the books hold
          on it stays, and comes back when you add the ledger again, but your
          node no longer settles on it meanwhile.
        </Alert>
        <h2>Prerequisites</h2>
        <Checklist items={removalChecklist(blockers())} />
        <Field
          label="Confirmation"
          for={`${id}-confirm`}
          hint={`Type ${expectedConfirmation(props.scheme)} to confirm.`}
        >
          <input
            id={`${id}-confirm`}
            data-testid="delete-confirmation"
            autocomplete="off"
            value={typed()}
            aria-describedby={`${id}-error`}
            onInput={(event) => setTyped(event.currentTarget.value)}
            onBlur={() => setTouched(true)}
          />
        </Field>
        <Show when={touched() && !confirmed()}>
          <p
            class="error"
            id={`${id}-error`}
            data-testid="delete-confirmation-error"
          >
            Confirmation must match exactly the expected format
          </p>
        </Show>
        <div class="actions">
          <button
            type="button"
            class="danger"
            data-testid="delete-submit"
            disabled={!confirmed() || blockers().length > 0 || pending()}
            onClick={remove}
          >
            Remove Ledger Account
          </button>
        </div>
        <ErrorText message={error()} testid="delete-error" />
      </Card>
    </div>
  );
}
