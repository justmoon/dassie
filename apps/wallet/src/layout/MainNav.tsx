/**
 * The main navigation: the pages, the debug pages in a menu, how many nodes
 * the node knows and whether the wallet is connected, the theme, and the
 * settings.
 */

import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { useNodeData } from "@dassie/ui/node-data";
import { useClient } from "@dassie/ui/rpc";
import { Link, useRouter } from "@dassie/ui/router";
import { ThemeToggle } from "@dassie/ui/theme-toggle";

const DEBUG_PAGES = [
  ["/debug/logs", "Logs"],
  ["/debug/nodes", "Nodes"],
  ["/debug/ledger", "Ledger"],
  ["/debug/routing", "Routing"],
  ["/debug/packets", "Packets"],
  ["/debug/state", "State"],
  ["/debug/database", "Database"],
] as const;

export function MainNav(): JSX.Element {
  const router = useRouter();
  const [open, setOpen] = createSignal(false);
  const [debugOpen, setDebugOpen] = createSignal(false);
  // A new page closes the menus.
  createEffect(() => router.path(), () => {
    setOpen(false);
    setDebugOpen(false);
  });
  return (
    <nav class="nav" aria-label="Main">
      <div class="nav-bar">
        <Link href="/" class="brand" end>Dassie Wallet</Link>
        <button
          type="button"
          class="nav-toggle"
          aria-expanded={open() ? "true" : "false"}
          aria-controls="nav-links"
          onClick={() => setOpen(!open())}
        >
          Menu
        </button>
      </div>
      <div id="nav-links" class={`nav-links ${open() ? "open" : ""}`}>
        <Link href="/dashboard" testid="nav-dashboard">Dashboard</Link>
        <Link href="/payments" testid="nav-activity">Activity</Link>
        <Link href="/grants" testid="nav-grants">Connected Apps</Link>
        <Link href="/network" testid="nav-network">Network</Link>
        <Link href="/ledgers" testid="nav-ledgers">Ledgers</Link>
        <div class="menu">
          <button
            type="button"
            aria-expanded={debugOpen() ? "true" : "false"}
            aria-controls="debug-menu"
            data-testid="nav-debug"
            onClick={() => setDebugOpen(!debugOpen())}
            onKeyDown={(event: KeyboardEvent) => {
              if (event.key === "Escape") setDebugOpen(false);
            }}
          >
            Debug
          </button>
          <Show when={debugOpen()}>
            <ul id="debug-menu" class="menu-list">
              <For each={DEBUG_PAGES}>
                {([href, title]) => (
                  <li>
                    <Link href={href}>{title}</Link>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
        <span class="nav-spacer" />
        <NetworkStatus />
        <ThemeToggle />
        <Link href="/settings" testid="nav-settings">Settings</Link>
      </div>
    </nav>
  );
}

/** How many nodes the node knows, and whether the wallet is connected. */
function NetworkStatus(): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const [connection, setConnection] = createSignal(client.connection);
  const stop = client.onConnectionChange(setConnection);
  onCleanup(stop);
  const state = () => {
    const status = data.nodesStatus();
    if (status.kind === "failed") return "failed";
    if (connection() === "waiting") return "reconnecting";
    return status.kind === "live" ? "connected" : "connecting";
  };
  const count = () => Object.keys(data.nodes.byId).length;
  return (
    <Link href="/debug/nodes" class="status" testid="network-status">
      <span
        class={`dot dot-${state()}`}
        data-state={state()}
        aria-hidden="true"
      />
      <span data-testid="node-count">
        {count()} {count() === 1 ? "node" : "nodes"}
      </span>
      <span class="sr-only">, {state()}</span>
    </Link>
  );
}
