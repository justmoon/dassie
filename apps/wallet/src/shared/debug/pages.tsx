/**
 * The debug pages (spec 6 §1.6.13, spec 7 §5): logs, nodes, ledger and its
 * accounts, routing, packets, the services and cells, and the database.
 * They read the node only through the RPC client and the node's live data
 * of their context, so the dev dashboard shows them for any node.
 */

import type { JSX } from "@solidjs/web";
import {
  type Account,
  type DatabaseValue,
  type Packet,
  PacketState,
  type ServiceState,
  TransferState,
} from "@dassie/rpc/proto";
import { createMemo, createSignal, For, Loading, Show } from "solid-js";
import { Amount } from "../amount/Amount.tsx";
import { type Currency, currencyOf } from "../amount/currency.ts";
import { dateTime, hex, prettyJson, shortId } from "../format.ts";
import { createLiveBuffer, createLiveValue } from "../live/live.ts";
import { LogViewer } from "../logs/LogViewer.tsx";
import { peerStateName, sortNodes } from "../network/nodes.ts";
import { useNodeData } from "../node-data.ts";
import { describe, useClient } from "../rpc.ts";
import { Link, type RouteProps, Routes } from "../router/Router.tsx";
import { ErrorText, LiveNote, PageTitle, Spinner } from "../ui/ui.tsx";
import {
  accountHref,
  accountRows,
  mergeTransfers,
  netBalance,
  splitPath,
} from "./ledger.ts";

/** How many lines the logs page keeps. */
const MAX_LOG_LINES = 10_000;

/** The currency of a ledger ID, from the node's ledgers or its name. */
function useCurrency(): (ledger: string) => Currency {
  const data = useNodeData();
  return (ledger) => {
    const known = data.ledgers()?.find((entry) => entry.id === ledger);
    if (known) return currencyOf(known.assetCode, known.assetScale);
    const code = ledger.split("+")[1]?.toUpperCase() ?? "?";
    return currencyOf(code, 9);
  };
}

const TABS = [
  ["/debug/logs", "Logs"],
  ["/debug/nodes", "Nodes"],
  ["/debug/ledger", "Ledger"],
  ["/debug/routing", "Routing"],
  ["/debug/packets", "Packets"],
  ["/debug/state", "State"],
  ["/debug/database", "Database"],
] as const;

/** The debug pages, with their own navigation. */
export function DebugPage(): JSX.Element {
  return (
    <div class="page-wide">
      <nav class="subnav" aria-label="Debug">
        <For each={TABS}>
          {([href, title]) => <Link href={href}>{title}</Link>}
        </For>
      </nav>
      <Loading fallback={<Spinner label="Loading…" />}>
        <Routes
          routes={[
            { path: "/debug/logs", component: LogsPage },
            { path: "/debug/nodes", component: NodesPage },
            { path: "/debug/ledger", component: LedgerPage },
            {
              path: "/debug/ledger/:ledger/account/*path",
              component: AccountPage,
            },
            { path: "/debug/routing", component: RoutingPage },
            { path: "/debug/packets", component: PacketsPage },
            { path: "/debug/state", component: StatePage },
            { path: "/debug/database", component: DatabasePage },
            { path: "/debug/database/:table", component: DatabasePage },
          ]}
          fallback={LogsPage}
        />
      </Loading>
    </div>
  );
}

export function LogsPage(): JSX.Element {
  const logs = createLiveBuffer(
    "subscribeLogs",
    () => ({}),
    (event) => event,
    MAX_LOG_LINES,
  );
  return (
    <>
      <PageTitle>Logs</PageTitle>
      <LiveNote status={logs.status()} />
      <LogViewer lines={logs.lines} />
    </>
  );
}

export function NodesPage(): JSX.Element {
  const data = useNodeData();
  const nodes = createMemo(() => sortNodes(Object.values(data.nodes.byId)));
  return (
    <>
      <PageTitle>Nodes</PageTitle>
      <LiveNote status={data.nodesStatus()} />
      <div class="table-scroll">
        <table data-testid="debug-nodes">
          <thead>
            <tr>
              <th scope="col">Node</th>
              <th scope="col">Alias</th>
              <th scope="col">Distance</th>
              <th scope="col">Next hops</th>
              <th scope="col">Neighbors</th>
              <th scope="col">Peering</th>
              <th scope="col">In the table as</th>
            </tr>
          </thead>
          <tbody>
            <For each={nodes()}>
              {(node) => (
                <tr data-testid={`debug-node-${node.id}`}>
                  <td>
                    <Link href={`/network/${node.id}`}>
                      <code>{shortId(node.id, 16)}</code>
                    </Link>
                    <div class="muted">{node.url || "no link state"}</div>
                  </td>
                  <td>{node.alias}</td>
                  <td>{node.distance ?? "∞"}</td>
                  <td>
                    {node.firstHops.map((hop) => shortId(hop, 8)).join(", ")}
                  </td>
                  <td>{node.neighbors.map((n) => shortId(n, 8)).join(", ")}</td>
                  <td>{peerStateName(node.peerState)}</td>
                  <td>{sources(node).join(", ")}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </>
  );
}

function sources(node: {
  isSelf: boolean;
  bootstrap: boolean;
  majority: boolean;
  registered: boolean;
  peer: boolean;
  banned: boolean;
}): string[] {
  const reasons: [boolean, string][] = [
    [node.isSelf, "self"],
    [node.bootstrap, "bootstrap"],
    [node.majority, "member"],
    [node.registered, "registered"],
    [node.peer, "peer"],
    [node.banned, "banned"],
  ];
  return reasons.filter(([on]) => on).map(([, name]) => name);
}

export function LedgerPage(): JSX.Element {
  const client = useClient();
  const currency = useCurrency();
  const [version, setVersion] = createSignal(0);
  const accounts = createMemo(async () => {
    version();
    const list = await client.call("listAccounts", {});
    return [...list.accounts].sort((a, b) => a.path.localeCompare(b.path));
  });
  const ledgerOf = (account: Account) => splitPath(account.path)?.ledger ?? "";
  return (
    <>
      <PageTitle>Ledger</PageTitle>
      <div class="actions">
        <button
          type="button"
          data-testid="debug-ledger-refresh"
          onClick={() => setVersion((n) => n + 1)}
        >
          Refresh
        </button>
      </div>
      <div class="table-scroll">
        <table data-testid="debug-ledger">
          <thead>
            <tr>
              <th scope="col">Account</th>
              <th scope="col">Balance</th>
              <th scope="col">Credits</th>
              <th scope="col">Debits</th>
            </tr>
          </thead>
          <tbody>
            <For each={accounts()}>
              {(account) => (
                <tr>
                  <td>
                    <Link
                      href={accountHref(account.path)}
                      testid={`account-link-${account.path}`}
                    >
                      {account.path}
                    </Link>
                  </td>
                  <td>
                    <Amount
                      value={netBalance(account)}
                      currency={currency(ledgerOf(account))}
                    />
                  </td>
                  <td>
                    <Amount
                      value={account.creditsPosted}
                      currency={currency(ledgerOf(account))}
                    />
                    <Show when={account.creditsPending > 0n}>
                      <span class="muted">
                        {` +${account.creditsPending} pending`}
                      </span>
                    </Show>
                  </td>
                  <td>
                    <Amount
                      value={account.debitsPosted}
                      currency={currency(ledgerOf(account))}
                    />
                    <Show when={account.debitsPending > 0n}>
                      <span class="muted">
                        {` +${account.debitsPending} pending`}
                      </span>
                    </Show>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </>
  );
}

export function AccountPage(props: RouteProps): JSX.Element {
  const client = useClient();
  const currency = useCurrency();
  const path = `${props.params.ledger}:${props.params.path}`;
  // Undefined while asking, null if there is no such account.
  const [id, setId] = createSignal<number | null>();
  client.call("listAccounts", {}).then(
    (list) =>
      setId(list.accounts.find((account) => account.path === path)?.id ?? null),
    () => setId(null),
  );
  return (
    <>
      <p class="breadcrumb">
        <Link href="/debug/ledger">{props.params.ledger}</Link> »{" "}
        <code>{props.params.path}</code>
      </p>
      <PageTitle>Account</PageTitle>
      <Show when={id() !== undefined} fallback={<Spinner label="Loading…" />}>
        <Show
          when={id()}
          keyed
          fallback={<p class="muted">No account at {path}.</p>}
        >
          {(account) => (
            <Transfers
              account={account}
              currency={currency(props.params.ledger)}
            />
          )}
        </Show>
      </Show>
    </>
  );
}

function Transfers(
  props: { account: number; currency: Currency },
): JSX.Element {
  const [held, setHeld] = createSignal<ReturnType<typeof mergeTransfers>>([]);
  const [account, setAccount] = createSignal<Account>();
  const live = createLiveValue(
    "subscribeTransfers",
    () => ({ account: props.account, history: 200 }),
    (event) => {
      setHeld((all) =>
        mergeTransfers(event.reset ? [] : all, event.transfers, 1_000)
      );
      if (event.account) setAccount(event.account);
      return event.transfers.length;
    },
  );
  const rows = createMemo(() => {
    const now = account();
    if (!now) return [];
    return accountRows(
      held(),
      props.account,
      now.creditsPosted - now.debitsPosted,
    )
      .reverse();
  });
  return (
    <>
      <LiveNote status={live.status()} />
      <Show
        when={rows().length > 0}
        fallback={
          <p class="muted" data-testid="transfers-empty">
            Waiting for transfers to occur...
          </p>
        }
      >
        <div class="table-scroll">
          <table data-testid="account-transfers">
            <thead>
              <tr>
                <th scope="col">Direction</th>
                <th scope="col">Counter-account</th>
                <th scope="col">Amount</th>
                <th scope="col">Balance</th>
                <th scope="col">When</th>
              </tr>
            </thead>
            <tbody>
              <For each={rows()}>
                {(row) => (
                  <tr
                    class={row.transfer.state === TransferState.PENDING
                      ? "is-pending"
                      : row.transfer.state === TransferState.VOIDED
                      ? "is-voided"
                      : ""}
                    data-testid="transfer-row"
                  >
                    <td>{row.credit ? "CR" : "DR"}</td>
                    <td>
                      <Link href={accountHref(row.counter)}>{row.counter}</Link>
                    </td>
                    <td>
                      <Amount
                        value={row.transfer.amount}
                        currency={props.currency}
                      />
                    </td>
                    <td>
                      <Amount value={row.balance} currency={props.currency} />
                    </td>
                    <td>{dateTime(row.transfer.createdAtUnixMs)}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
    </>
  );
}

export function RoutingPage(): JSX.Element {
  const routes = createLiveValue("subscribeRoutes", () => ({}), (item) => item);
  return (
    <>
      <PageTitle>Routing</PageTitle>
      <LiveNote status={routes.status()} />
      <div class="table-scroll">
        <table data-testid="debug-routes">
          <thead>
            <tr>
              <th scope="col">ILP prefix</th>
              <th scope="col">Type</th>
              <th scope="col">Details</th>
            </tr>
          </thead>
          <tbody>
            <For each={routes.value()?.localRoutes ?? []}>
              {(route) => (
                <tr>
                  <td>
                    <code>{route.prefix}</code>
                  </td>
                  <td>local</td>
                  <td>{route.endpoint} ({route.ledger})</td>
                </tr>
              )}
            </For>
            <For each={routes.value()?.routes ?? []}>
              {(route) => (
                <tr data-testid="route-row">
                  <td>
                    <code>{route.prefix}</code>
                  </td>
                  <td>peer</td>
                  <td>
                    {route.firstHops.map((hop) => shortId(hop, 10)).join(", ")}
                    {" "}
                    (distance: {route.distance})
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </>
  );
}

export function PacketsPage(): JSX.Element {
  const client = useClient();
  const [packets, setPackets] = createSignal<Packet[]>([]);
  const [lagged, setLagged] = createSignal(0n);
  const key = (packet: Packet) =>
    `${packet.trace?.epoch}:${packet.trace?.sequence}`;
  const add = (incoming: readonly Packet[]) =>
    setPackets((all) => {
      const byKey = new Map(all.map((packet) => [key(packet), packet]));
      for (const packet of incoming) byKey.set(key(packet), packet);
      return [...byKey.values()].slice(-500);
    });
  client.call("listPackets", { limit: 200 }).then(
    (list) => add(list.packets),
    () => {},
  );
  const live = createLiveValue("subscribePackets", () => ({}), (item) => {
    const event = item.event;
    if (event.case === "packet") add([event.value]);
    if (event.case === "lagged") {
      const missed = event.value;
      setLagged((n) => n + missed);
    }
    return item;
  });
  const state = (packet: Packet) =>
    packet.state === PacketState.PENDING
      ? "pending"
      : packet.state === PacketState.FULFILLED
      ? "fulfilled"
      : `rejected ${packet.rejectCode}`;
  return (
    <>
      <PageTitle>Packets</PageTitle>
      <LiveNote status={live.status()} />
      <Show when={lagged() > 0n}>
        <p class="muted">{lagged().toString()} packets were missed.</p>
      </Show>
      <div class="table-scroll">
        <table data-testid="debug-packets">
          <thead>
            <tr>
              <th scope="col">From</th>
              <th scope="col">To</th>
              <th scope="col">Destination</th>
              <th scope="col">Amount in</th>
              <th scope="col">Amount out</th>
              <th scope="col">State</th>
            </tr>
          </thead>
          <tbody>
            <For each={[...packets()].reverse()}>
              {(packet) => (
                <tr>
                  <td>{packet.source}</td>
                  <td>{packet.destination}</td>
                  <td>
                    <code>{packet.destinationAddress}</code>
                  </td>
                  <td>{packet.amountIn.toString()}</td>
                  <td>{packet.amountOut.toString()}</td>
                  <td>{state(packet)}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </>
  );
}

export function StatePage(): JSX.Element {
  const client = useClient();
  const [version, setVersion] = createSignal(0);
  const state = createMemo(async () => {
    version();
    return await client.call("listServices", {});
  });
  const [cell, setCell] = createSignal<string>();
  const stateName = (state: ServiceState) =>
    ["unknown", "running", "restarting", "stopped"][state] ?? "unknown";
  return (
    <div data-testid="debug-state">
      <PageTitle>State</PageTitle>
      <div class="actions">
        <button type="button" onClick={() => setVersion((n) => n + 1)}>
          Refresh
        </button>
      </div>
      <h2>Services</h2>
      <div class="table-scroll">
        <table>
          <thead>
            <tr>
              <th scope="col">Service</th>
              <th scope="col">State</th>
              <th scope="col">Started</th>
              <th scope="col">Restarts</th>
              <th scope="col">Last failure</th>
            </tr>
          </thead>
          <tbody>
            <For each={state().services}>
              {(service) => (
                <tr data-testid={`service-${service.name}`}>
                  <td>{service.name}</td>
                  <td>{stateName(service.state)}</td>
                  <td>{dateTime(service.startedAtUnixMs)}</td>
                  <td>{service.restarts}</td>
                  <td>{service.lastError || "—"}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
      <h2>Cells</h2>
      <ul class="cells">
        <For each={state().cells}>
          {(entry) => (
            <li>
              <button
                type="button"
                class="link"
                aria-expanded={cell() === entry.name ? "true" : "false"}
                onClick={() =>
                  setCell(cell() === entry.name ? undefined : entry.name)}
              >
                {entry.name}
              </button>{" "}
              <span class="muted">version {entry.version.toString()}</span>
              <Show when={cell() === entry.name}>
                <pre>{prettyJson(entry)}</pre>
              </Show>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}

export function DatabasePage(props: RouteProps): JSX.Element {
  const client = useClient();
  const tables = createMemo(async () =>
    (await client.call("listDatabaseTables", {})).tables
  );
  return (
    <>
      <PageTitle>Database</PageTitle>
      <div class="split">
        <ul class="table-list">
          <For each={tables()}>
            {(table) => (
              <li>
                <Link
                  href={`/debug/database/${table.name}`}
                  testid={`db-table-${table.name}`}
                >
                  {table.name}
                </Link>{" "}
                <span class="muted">
                  {table.rowsExact ? "" : "≤ "}
                  {table.rows.toString()}
                </span>
              </li>
            )}
          </For>
        </ul>
        <Show when={props.params.table} keyed>
          {(name) => (
            <Rows
              name={name}
              columns={tables().find((table) => table.name === name)?.columns ??
                []}
            />
          )}
        </Show>
      </div>
    </>
  );
}

function Rows(props: {
  name: string;
  columns: readonly { name: string; redacted: boolean }[];
}): JSX.Element {
  const client = useClient();
  const [cursors, setCursors] = createSignal<bigint[]>([0n]);
  const loaded = createMemo(async () => {
    const cursor = cursors().at(-1) ?? 0n;
    try {
      const page = await client.call("listDatabaseRows", {
        table: props.name,
        cursor,
        limit: 50,
      });
      return { page, error: undefined };
    } catch (failure) {
      return { page: undefined, error: describe(failure) };
    }
  });
  const page = () => loaded().page;
  const show = (value: DatabaseValue) => {
    const v = value.value;
    switch (v.case) {
      case "integer":
        return v.value.toString();
      case "text":
        return v.value;
      case "blob":
        return hex(v.value);
      case "redacted":
        return "[redacted]";
      default:
        return "NULL";
    }
  };
  return (
    <div class="table-scroll" data-testid="db-rows">
      <ErrorText message={loaded().error} />
      <table>
        <thead>
          <tr>
            <For each={props.columns}>
              {(column) => (
                <th scope="col">
                  {column.name}
                  {column.redacted ? " (secret)" : ""}
                </th>
              )}
            </For>
          </tr>
        </thead>
        <tbody>
          <For each={page()?.rows ?? []}>
            {(row) => (
              <tr data-testid="db-row">
                <For each={row.values}>
                  {(value) => (
                    <td>
                      {show(value)}
                      {value.fullLength > 0n ? "…" : ""}
                    </td>
                  )}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
      <div class="actions">
        <button
          type="button"
          disabled={cursors().length <= 1}
          onClick={() => setCursors((all) => all.slice(0, -1))}
        >
          Previous
        </button>
        <button
          type="button"
          disabled={!page()?.nextCursor}
          onClick={() => {
            const next = page()?.nextCursor;
            if (next) setCursors((all) => [...all, next]);
          }}
        >
          Next
        </button>
      </div>
    </div>
  );
}
