/**
 * Settings (spec/areas/wallet.md#pages): the wallet, the node's version and
 * logging out, the theme, the node's alias, where received funds are kept,
 * what transit is charged and the node's swaps with its peers, exchange
 * rates and the display currency, the owner's sessions and paired devices,
 * and the BTP tokens with their caps.
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
import { Devices } from "./Devices.tsx";
import { Funds } from "./Funds.tsx";
import { RateSettings } from "./RateSettings.tsx";
import { chargeText, parseCharge } from "./rates.ts";
import { assetLedgers, targetsFrom, targetText } from "./swaps.ts";
import { useStatus } from "../../status.ts";

export function Settings(): JSX.Element {
  return (
    <div class="page-wide">
      <PageTitle>Settings</PageTitle>
      <div class="grid-2">
        <div class="stack">
          <Wallet />
          <Appearance />
          <NodeSettings />
          <Funds />
          <PeerSettings />
          <RateSettings />
        </div>
        <div class="stack">
          <Sessions />
          <Devices />
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
        <dt>Dassie version</dt>
        <dd>
          <code data-testid="settings-version">{data.version}</code>
        </dd>
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

/** What passing a packet from one peer to another is charged
 * (spec/decisions/exchange-rates.md), how far the owner's inventory moves
 * what it converts at (spec/decisions/passive-rebalancing.md), and the
 * node's swaps with its peers, with a periphery node's targets
 * (spec/decisions/peer-swaps.md). */
function PeerSettings(): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const data = useNodeData();
  const status = useStatus();
  const id = createUniqueId();
  const [version, setVersion] = createSignal(0);
  const settings = createMemo(async () => {
    version();
    return (await client.call("getPeerSettings", {})).settings;
  });
  const [fee, setFee] = createSignal<string>();
  const [spread, setSpread] = createSignal<string>();
  const [rebalance, setRebalance] = createSignal<string>();
  const [offer, setOffer] = createSignal<boolean>();
  const [swapping, setSwapping] = createSignal<boolean>();
  const [typed, setTyped] = createSignal<Record<string, string>>({});
  const [error, setError] = createSignal<string>();
  const shownFee = () =>
    fee() ?? (settings() ? chargeText(settings()!.connectorFeePpm) : "");
  const shownSpread = () =>
    spread() ?? (settings() ? chargeText(settings()!.fxSpreadPpm) : "");
  const shownRebalance = () =>
    rebalance() ??
      (settings() ? chargeText(settings()!.rebalancePpm) : "");
  const shownOffer = () => offer() ?? settings()?.offerSwaps ?? true;
  const shownSwapping = () => swapping() ?? settings()?.swapping ?? true;
  const periphery = () => status()?.periphery === true;
  const assets = () =>
    assetLedgers((data.ledgers() ?? []).map((ledger) => ledger.id));
  const shownTarget = (ledger: string) => {
    const own = typed()[ledger];
    if (own !== undefined) return own;
    const kept = settings()?.swapTargets.find((t) => t.ledger === ledger);
    return kept ? targetText(kept.targetPpm) : "";
  };
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    setError(undefined);
    const [feePpm, spreadPpm, rebalancePpm] = [
      parseCharge(shownFee()),
      parseCharge(shownSpread()),
      parseCharge(shownRebalance()),
    ];
    if (
      feePpm === undefined || spreadPpm === undefined ||
      rebalancePpm === undefined
    ) {
      setError("A fee, a spread or a rebalancing is at most 10%.");
      return;
    }
    // A core node keeps the targets it has: only a periphery node aims for
    // them.
    const targets = periphery()
      ? targetsFrom(assets().map((ledger) => [ledger, shownTarget(ledger)]))
      : { targets: settings()?.swapTargets ?? [] };
    if ("invalid" in targets) {
      setError(
        `The target for ${targets.invalid} is more than 0 and at most 100%.`,
      );
      return;
    }
    try {
      await client.call("setPeerSettings", {
        settings: {
          connectorFeePpm: feePpm,
          fxSpreadPpm: spreadPpm,
          rebalancePpm,
          offerSwaps: shownOffer(),
          swapping: shownSwapping(),
          swapTargets: targets.targets,
        },
      });
      toast({ title: "Peer settings saved" });
      setFee(undefined);
      setSpread(undefined);
      setRebalance(undefined);
      setOffer(undefined);
      setSwapping(undefined);
      setTyped({});
      setVersion((n) => n + 1);
    } catch (failure) {
      setError(describe(failure));
    }
  };
  return (
    <Card title="Peers" testid="peer-settings">
      <form onSubmit={save}>
        <Field
          label="Fee (% of what passes)"
          for={`${id}-fee`}
          hint="Charged on what you pass from one peer to another, never on your own payments."
        >
          <input
            id={`${id}-fee`}
            data-testid="connector-fee-input"
            inputmode="decimal"
            value={shownFee()}
            onInput={(event) => setFee(event.currentTarget.value)}
          />
        </Field>
        <Field
          label="Spread (% of what is converted)"
          for={`${id}-spread`}
          hint="Kept from what you convert from one currency to another for peers."
        >
          <input
            id={`${id}-spread`}
            data-testid="fx-spread-input"
            inputmode="decimal"
            value={shownSpread()}
            onInput={(event) => setSpread(event.currentTarget.value)}
          />
        </Field>
        <Field
          label="Rebalancing (%)"
          for={`${id}-rebalance`}
          hint="How far your holdings move the rate you convert at for peers: up to this much better for what you are short of, and worse for taking it out. 0 converts at the market rate."
        >
          <input
            id={`${id}-rebalance`}
            data-testid="rebalance-input"
            inputmode="decimal"
            value={shownRebalance()}
            onInput={(event) => setRebalance(event.currentTarget.value)}
          />
        </Field>
        <Show when={!periphery()}>
          <Field
            label="Swaps for peers"
            for={`${id}-offer`}
            hint="Whether peers may swap one currency for another with you, at the rates above."
          >
            <select
              id={`${id}-offer`}
              data-testid="offer-swaps-input"
              onChange={(event) => setOffer(event.currentTarget.value === "on")}
            >
              <option value="on" selected={shownOffer()}>Offered</option>
              <option value="off" selected={!shownOffer()}>Not offered</option>
            </select>
          </Field>
        </Show>
        <Field
          label="Swapping with peers"
          for={`${id}-swapping`}
          hint={periphery()
            ? "Whether your node reads its uplinks' quotes, to swap toward the targets below and when you ask."
            : "Whether your node swaps with its peers by itself, toward an even split of what it holds, when their quotes beat its costs."}
        >
          <select
            id={`${id}-swapping`}
            data-testid="swapping-input"
            onChange={(event) =>
              setSwapping(event.currentTarget.value === "on")}
          >
            <option value="on" selected={shownSwapping()}>On</option>
            <option value="off" selected={!shownSwapping()}>Off</option>
          </select>
        </Field>
        <Show when={periphery()}>
          <p class="muted">
            Targets (% of what you hold in the currencies with one): your node
            swaps by itself only toward targets you set. Leave all empty to swap
            only when you ask.
          </p>
          <For each={assets()}>
            {(ledger) => (
              <Field label={ledger} for={`${id}-target-${ledger}`}>
                <input
                  id={`${id}-target-${ledger}`}
                  data-testid={`swap-target-${ledger}`}
                  inputmode="decimal"
                  value={shownTarget(ledger)}
                  onInput={(event) => {
                    const text = event.currentTarget.value;
                    setTyped((all) => ({ ...all, [ledger]: text }));
                  }}
                />
              </Field>
            )}
          </For>
        </Show>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-testid="peer-settings-save"
          >
            Save
          </button>
        </div>
      </form>
      <ErrorText message={error()} />
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
