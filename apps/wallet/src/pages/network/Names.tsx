/**
 * Names (spec/areas/wallet.md#pages): registering a name for this node at
 * every registrar, how each name stands with each of them, and renewing the
 * names the node holds (spec/areas/network.md#the-registrant).
 */

import type { JSX } from "@solidjs/web";
import { createSignal, createUniqueId, For, Show } from "solid-js";
import { relativeTime, shortId } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import {
  Alert,
  Badge,
  Card,
  ErrorText,
  Field,
  LiveNote,
  PageTitle,
} from "@dassie/ui/ui";
import type { NamesStatus, OwnName } from "@dassie/rpc/proto";
import {
  atRegistrar,
  nameHint,
  nameState,
  registrable,
  renewable,
} from "./names.ts";

export function Names(): JSX.Element {
  const status = createLiveValue(
    "subscribeNames",
    () => ({}),
    (item) => item,
  );
  return (
    <div class="page-narrow stack">
      <PageTitle>Names</PageTitle>
      <LiveNote status={status.status()} />
      <Show when={status.value()}>
        {(names) => (
          <Show when={!names().off} fallback={<NamesOff />}>
            <RegisterName />
            <NameList names={names()} />
          </Show>
        )}
      </Show>
    </div>
  );
}

/** What the page says while names are off: in the live realm. */
function NamesOff(): JSX.Element {
  return (
    <Card title="Names" testid="names-off">
      <p class="muted">
        Names run on the testnet only for now, where they may be reset at any
        time. A node in the live realm registers none and holds none until
        registering a name has a fee.
      </p>
    </Card>
  );
}

/** The form that registers a name. */
function RegisterName(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const id = createUniqueId();
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const register = async (event: SubmitEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      await client.call("registerName", { name: name() });
      toast({ title: "Registering", description: name() });
      setName("");
    } catch (failure) {
      setError(describe(failure));
    }
    setBusy(false);
  };
  return (
    <Card title="Register a Name" testid="register-name">
      <p class="muted">
        A name is held for your node by the network's bootstrap nodes, its
        registrars. Your node commits to it sealed at every registrar, and tells
        them the name only once each has logged the commitment, so that nobody
        can take it from you after seeing it. It is yours once their windows
        close, and your node renews it before it expires.
      </p>
      <form onSubmit={register}>
        <Field
          label="Name"
          for={`${id}-name`}
          hint="3 to 32 lowercase letters, digits and hyphens."
        >
          <input
            id={`${id}-name`}
            data-testid="name-input"
            value={name()}
            maxlength="32"
            autocomplete="off"
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </Field>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-testid="name-register"
            disabled={busy() || name() === ""}
          >
            Register
          </button>
        </div>
      </form>
      <ErrorText message={error()} testid="name-error" />
    </Card>
  );
}

/** The names the node registers or holds. */
function NameList(props: { names: NamesStatus }): JSX.Element {
  return (
    <Card title="Your Names" testid="names">
      <Show when={props.names.frozen}>
        <Alert title="The registrars' lists are not current" tone="warning">
          Too few of the registrars' lists are fresh: your node keeps the
          holders it last knew.
        </Alert>
      </Show>
      <Show when={props.names.registrars.length === 0}>
        <p class="muted">
          This node has no bootstrap nodes, so no registrars: it registers names
          once <code>DASSIE_BOOTSTRAP_NODES</code> names some.
        </p>
      </Show>
      <Show
        when={props.names.names.length > 0}
        fallback={<p class="muted">No names yet.</p>}
      >
        <ul class="name-list">
          <For each={props.names.names}>
            {(name) => <NameEntry name={name} node={props.names.node} />}
          </For>
        </ul>
      </Show>
    </Card>
  );
}

/** One name: how it stands, with each registrar, and its actions. */
function NameEntry(props: { name: OwnName; node: string }): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const act = async (method: "renewName" | "registerName", done: string) => {
    setBusy(true);
    setError(undefined);
    try {
      await client.call(method, { name: props.name.name });
      toast({ title: done, description: props.name.name });
    } catch (failure) {
      setError(describe(failure));
    }
    setBusy(false);
  };
  const state = () => nameState(props.name.state);
  const holder = () => props.name.holder;
  return (
    <li data-testid={`name-${props.name.name}`}>
      <div class="name-head">
        <strong class="name-title">{props.name.name}</strong>{" "}
        <Badge tone={state().tone} testid={`name-state-${props.name.name}`}>
          {state().text}
        </Badge>
      </div>
      <Show when={nameHint(props.name.state)}>
        {(hint) => <p class="muted">{hint()}</p>}
      </Show>
      <Show when={holder() !== "" && holder() !== props.node}>
        <p class="muted">
          Held by <code>{shortId(holder(), 20)}</code>
        </p>
      </Show>
      <Show when={props.name.expiresAtUnixMs !== 0n}>
        <p class="muted">
          Expires {relativeTime(props.name.expiresAtUnixMs, Date.now())}
        </p>
      </Show>
      <Show when={props.name.unsealByUnixMs !== 0n}>
        <p class="muted">
          Every registrar must log the commitment{" "}
          {relativeTime(props.name.unsealByUnixMs, Date.now())}
        </p>
      </Show>
      <ul class="registrar-list">
        <For each={props.name.registrars}>
          {(at) => (
            <li data-testid={`registrar-${props.name.name}-${at.registrar}`}>
              <code>{shortId(at.registrar, 20)}</code>{" "}
              <span class="muted">
                {atRegistrar(at, props.node, Date.now())}
              </span>
            </li>
          )}
        </For>
      </ul>
      <div class="actions">
        <Show when={renewable(props.name.state)}>
          <button
            type="button"
            data-testid={`renew-${props.name.name}`}
            disabled={busy()}
            onClick={() => act("renewName", "Renewing")}
          >
            Renew now
          </button>
        </Show>
        <Show when={registrable(props.name.state)}>
          <button
            type="button"
            data-testid={`register-again-${props.name.name}`}
            disabled={busy()}
            onClick={() => act("registerName", "Registering")}
          >
            Register again
          </button>
        </Show>
      </div>
      <ErrorText message={error()} />
    </li>
  );
}
