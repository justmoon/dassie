/**
 * The sidebar (spec 6 §2): the pages, then the nodes, each with its color,
 * its state and a link that opens its wallet signed in.
 */

import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import type { Node } from "@dassie/rpc/dev";
import { seedIndex } from "@dassie/ui/network/nodes";
import { Link, useRouter } from "@dassie/ui/router";
import { ThemeToggle } from "@dassie/ui/theme-toggle";
import { nodePath, stateName, walletLink } from "./nodes.ts";
import { useDev } from "./store.ts";

/** The brand's suffix, as the original's: Dassie//dev. */
const BRAND_SUFFIX = "//dev";

export function Sidebar(): JSX.Element {
  const dev = useDev();
  return (
    <nav class="dev-sidebar" aria-label="Dashboard">
      <p class="dev-brand">
        Dassie<span class="dev-brand-suffix">{BRAND_SUFFIX}</span>
      </p>
      <ul class="dev-links">
        <li>
          <Link href="/" end testid="nav-dashboard">Dashboard</Link>
        </li>
        <li>
          <Link href="/logs" testid="nav-logs">Logs</Link>
        </li>
        <li>
          <Link href="/scenarios" testid="nav-scenarios">Scenarios</Link>
        </li>
      </ul>
      <div class="dev-nodes-head">
        <h2>Nodes</h2>
        <button
          type="button"
          class="icon"
          data-testid="add-node"
          aria-label="Add a node"
          title="Add a node"
          onClick={() => dev.send({ case: "addNode", value: {} })}
        >
          +
        </button>
      </div>
      <ul class="dev-nodes" data-testid="node-list">
        <li>
          <Link href="/nodes/host" testid="node-host">
            <span class={`dot seed-${seedIndex("host")}`} aria-hidden="true" />
            {" "}
            host
          </Link>
        </li>
        <For each={dev.state().nodes} keyed={(node) => node.name}>
          {(node) => <NodeEntry node={node()} />}
        </For>
      </ul>
      <div class="dev-sidebar-foot">
        <ThemeToggle />
      </div>
    </nav>
  );
}

/** A node: its page, keeping the debug tab shown, and its wallet. */
function NodeEntry(props: { node: Node }): JSX.Element {
  const router = useRouter();
  const target = () => nodePath(props.node.name, router.path());
  return (
    <li
      data-testid={`node-${props.node.name}`}
      data-state={stateName(props.node.state)}
    >
      <a
        href={router.href(target())}
        aria-current={router.path().startsWith(`/nodes/${props.node.name}/`) ||
            router.path() === `/nodes/${props.node.name}`
          ? "page"
          : undefined}
        onClick={(event: MouseEvent) => {
          if (event.button !== 0 || event.metaKey || event.ctrlKey) return;
          event.preventDefault();
          router.navigate(target());
        }}
      >
        <span
          class={`dot seed-${seedIndex(props.node.name)}`}
          aria-hidden="true"
        />{" "}
        {props.node.name}{" "}
        <span class={`muted node-state state-${props.node.state}`}>
          {stateName(props.node.state)}
        </span>
      </a>
      <a
        class="wallet-link"
        href={walletLink(props.node)}
        target="_blank"
        rel="noopener"
        data-testid={`wallet-${props.node.name}`}
        aria-label={`Open ${props.node.name}'s wallet`}
        title="Open its wallet, signed in"
      >
        ↗
      </a>
    </li>
  );
}
