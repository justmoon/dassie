/**
 * Removing a ledger of an asset (spec/areas/wallet.md#pages): what blocks
 * it, as the node reports it live, and a typed confirmation. It goes back
 * to the asset's page, or to Assets once the asset has no other ledger.
 */

import type { JSX } from "@solidjs/web";
import { createSignal, createUniqueId, Show } from "solid-js";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Link, Navigate, type RouteProps, useRouter } from "@dassie/ui/router";
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
import {
  expectedConfirmation,
  removalChecklist,
  removingNotice,
} from "./removal.ts";

export function DeleteLedger(props: RouteProps): JSX.Element {
  const data = useNodeData();
  const ledger = () =>
    data.ledgers()?.find((entry) =>
      entry.settlementScheme === props.params.scheme
    );
  return (
    <Show when={ledger()} fallback={<Navigate href="/assets" />}>
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
  const ledger = () =>
    data.ledgers()?.find((entry) => entry.settlementScheme === props.scheme);
  const blockers = () => ledger()?.removalBlockers ?? [];
  const removing = () => removingNotice(ledger()?.removingMethods);
  const confirmed = () => typed() === expectedConfirmation(props.scheme);
  // The asset it is a ledger of, while it has another.
  const back = () => {
    const id = ledger()?.id;
    const asset = data.balances().find((balance) =>
      id !== undefined && balance.ledgers.includes(id)
    );
    return asset && asset.ledgers.length > 1
      ? `/assets/${asset.name}`
      : "/assets";
  };
  const remove = async () => {
    setPending(true);
    setError(undefined);
    const to = back();
    try {
      await client.call("removeSettlementScheme", { id: props.scheme });
      // It goes at once, or once its peers have settled up.
      toast({ title: `Removing ${schemeName(props.scheme)}` });
      router.navigate(to);
    } catch (failure) {
      setError(describe(failure));
      setPending(false);
    }
  };
  return (
    <div class="page-narrow">
      <p class="breadcrumb">
        <Link href="/assets">Assets</Link> ›
      </p>
      <PageTitle>Removing ledger {schemeName(props.scheme)}</PageTitle>
      <Card testid="page-delete-ledger" class="ledger-removal">
        <Alert title="Possibility to lose funds" tone="danger">
          Removing a ledger stops your node from using it. What the books hold
          on it stays, and comes back when you add the ledger again, but your
          node no longer settles on it meanwhile.
        </Alert>
        <Show when={removing()}>
          {(notice) => (
            <Alert title={notice().title} testid="delete-removing">
              {notice().text}
            </Alert>
          )}
        </Show>
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
            disabled={!confirmed() || blockers().length > 0 ||
              removing() !== undefined || pending()}
            onClick={remove}
          >
            Remove ledger account
          </button>
        </div>
        <ErrorText message={error()} testid="delete-error" />
      </Card>
    </div>
  );
}
