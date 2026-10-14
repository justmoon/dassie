/**
 * Settings (spec 6 §1.6.12): the wallet and logging out, the theme, the
 * node's alias, the owner's sessions, and the BTP tokens with their caps.
 */

import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createUniqueId, For, Show } from "solid-js";
import { submitLogout } from "../../auth.ts";
import { useTheme } from "@dassie/ui/theme-toggle";
import { logoutFailure } from "../../messages.ts";
import { dateTime } from "@dassie/ui/format";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import {
  Badge,
  Card,
  ConfirmButton,
  ErrorText,
  Field,
  PageTitle,
} from "@dassie/ui/ui";
import type { ThemeSetting } from "@dassie/ui/theme";
import { ApiKeys } from "./ApiKeys.tsx";

export function Settings(): JSX.Element {
  return (
    <div class="page-wide">
      <PageTitle>Settings</PageTitle>
      <div class="grid-2">
        <div class="stack">
          <Wallet />
          <Appearance />
          <NodeSettings />
        </div>
        <div class="stack">
          <Sessions />
          <ApiKeys />
        </div>
      </div>
    </div>
  );
}

function Wallet(): JSX.Element {
  const data = useNodeData();
  const [error, setError] = createSignal<string>();
  const logout = async () => {
    const outcome = await submitLogout();
    if (outcome.ok) location.reload();
    else setError(logoutFailure(outcome));
  };
  return (
    <Card title="Wallet" testid="settings-wallet">
      <p class="muted">Manage your account</p>
      <dl class="facts">
        <dt>Node ID</dt>
        <dd>
          <code data-testid="settings-node-id">{data.selfId}</code>
        </dd>
        <dt>Payment Pointer</dt>
        <dd>{data.hostname ? `$${data.hostname}` : "???"}</dd>
      </dl>
      <div class="actions">
        <button
          type="button"
          class="danger"
          data-testid="logout"
          onClick={logout}
        >
          Logout
        </button>
      </div>
      <ErrorText message={error()} testid="logout-error" />
    </Card>
  );
}

function Appearance(): JSX.Element {
  const [theme, setTheme] = useTheme();
  const options: [ThemeSetting, string][] = [
    ["system", "As the system"],
    ["light", "Light"],
    ["dark", "Dark"],
  ];
  return (
    <Card title="Appearance">
      <fieldset class="radios">
        <legend>Theme</legend>
        <For each={options}>
          {([value, label]) => (
            <label>
              <input
                type="radio"
                name="theme"
                value={value}
                data-testid={`theme-${value}`}
                checked={theme() === value}
                onChange={() => setTheme(value)}
              />{" "}
              {label}
            </label>
          )}
        </For>
      </fieldset>
    </Card>
  );
}

function NodeSettings(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const id = createUniqueId();
  const [version, setVersion] = createSignal(0);
  const config = createMemo(async () => {
    version();
    return (await client.call("getConfig", {})).config;
  });
  const [alias, setAlias] = createSignal<string>();
  const [error, setError] = createSignal<string>();
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    setError(undefined);
    try {
      await client.call("setAlias", {
        alias: alias() ?? config()?.alias ?? "",
      });
      toast({ title: "Alias saved" });
      setVersion((n) => n + 1);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <Card title="Node Settings" testid="node-settings">
      <p class="muted">Edit your node settings</p>
      <form onSubmit={save}>
        <Field
          label="Alias"
          for={`${id}-alias`}
          hint="Shown to other nodes; at most 64 bytes."
        >
          <input
            id={`${id}-alias`}
            data-testid="alias-input"
            value={alias() ?? config()?.alias ?? ""}
            maxlength="64"
            onInput={(event) => setAlias(event.currentTarget.value)}
          />
        </Field>
        <div class="actions">
          <button type="submit" class="primary" data-testid="alias-save">
            Save
          </button>
        </div>
      </form>
      <ErrorText message={error()} />
      <dl class="facts">
        <dt>Hostname</dt>
        <dd>
          {config()?.hostname || "not set"}{" "}
          <span class="muted">
            (change it with <code>dassie init</code>)
          </span>
        </dd>
      </dl>
    </Card>
  );
}

function Sessions(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const [version, setVersion] = createSignal(0);
  const sessions = createMemo(async () => {
    version();
    return (await client.call("listSessions", {})).sessions;
  });
  const revoke = async (id: string, current: boolean) => {
    try {
      await client.call("revokeSession", { id });
      // The node closes this socket when our own session ends.
      if (current) location.reload();
      toast({ title: "Session ended" });
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
    <Card title="Sessions" testid="sessions">
      <p class="muted">Browsers signed in to your node</p>
      <ul class="sessions">
        <For each={sessions()}>
          {(session) => (
            <li data-testid={`session-${session.id}`}>
              <div>
                {session.userAgent || "unknown browser"}{" "}
                <Show when={session.current}>
                  <Badge tone="good" testid="session-current">
                    This browser
                  </Badge>
                </Show>
              </div>
              <div class="muted">
                since {dateTime(session.createdAtUnixMs)}, last used{" "}
                {dateTime(session.lastUsedAtUnixMs)}
              </div>
              <ConfirmButton
                label="Revoke"
                title="End this session?"
                confirm="End it"
                tone="danger"
                testid={`revoke-session-${session.id}`}
                onConfirm={() => revoke(session.id, session.current)}
              >
                <p>The browser is logged out at once.</p>
              </ConfirmButton>
            </li>
          )}
        </For>
      </ul>
      <div class="actions">
        <button
          type="button"
          data-testid="revoke-others"
          onClick={async () => {
            try {
              const { revoked } = await client.call("revokeAllSessions", {
                keepCurrent: true,
              });
              toast({ title: `Logged out ${revoked} other sessions` });
              setVersion((n) => n + 1);
            } catch (failure) {
              toast({
                title: "That did not work",
                description: describe(failure),
                tone: "destructive",
              });
            }
          }}
        >
          Log out all other sessions
        </button>
      </div>
    </Card>
  );
}
