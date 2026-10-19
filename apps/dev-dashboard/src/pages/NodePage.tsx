/**
 * One node (spec/areas/dev.md#dashboard): its wallet link, and the wallet's
 * debug pages, talking to this node through the dev server's relay to its IPC
 * socket. The pages live under `/nodes/dN/debug/…`.
 */

import type { JSX } from "@solidjs/web";
import { createSignal, onCleanup, Show } from "solid-js";
import { RpcClient } from "@dassie/rpc";
import type { GetBasicStateResponse } from "@dassie/rpc/proto";
import { NodeState } from "@dassie/rpc/dev";
import { DebugPage } from "@dassie/ui/debug";
import { seedIndex } from "@dassie/ui/network/nodes";
import { createNodeData, NodeDataContext } from "@dassie/ui/node-data";
import { type RouteProps, Router } from "@dassie/ui/router";
import { describe, RpcContext } from "@dassie/ui/rpc";
import { Alert, PageTitle, Spinner } from "@dassie/ui/ui";
import { stateName, walletLink } from "../nodes.ts";
import { useDev } from "../store.ts";

export function NodePage(props: RouteProps): JSX.Element {
  return (
    <Show when={props.params.node} keyed>
      {(name) => <NodeView name={name} />}
    </Show>
  );
}

function NodeView(props: { name: string }): JSX.Element {
  const dev = useDev();
  const node = () => dev.state().nodes.find((node) => node.name === props.name);
  const running = () => node()?.state === NodeState.RUNNING;
  return (
    <div class="page-wide" data-testid="page-node">
      <div class="node-head">
        <PageTitle>
          <span
            class={`dot seed-${seedIndex(props.name)}`}
            aria-hidden="true"
          />{" "}
          Node: {props.name}
        </PageTitle>
        <Show when={node()}>
          {(known) => (
            <a
              class="button"
              href={walletLink(known())}
              target="_blank"
              rel="noopener"
              data-testid="node-wallet"
            >
              Wallet ↗
            </a>
          )}
        </Show>
      </div>
      <Show when={running()} keyed fallback={<NotRunning name={props.name} />}>
        <Connected name={props.name} />
      </Show>
    </div>
  );
}

function NotRunning(props: { name: string }): JSX.Element {
  const dev = useDev();
  const node = () => dev.state().nodes.find((node) => node.name === props.name);
  return (
    <Show
      when={node()}
      fallback={
        <p data-testid="node-unknown">There is no node {props.name}.</p>
      }
    >
      {(known) => (
        <div data-testid="node-not-running">
          <p>{props.name} is {stateName(known().state)}.</p>
          <Show when={known().lastError}>
            <Alert title="Its last words" tone="danger">
              <pre>{known().lastError}</pre>
            </Alert>
          </Show>
        </div>
      )}
    </Show>
  );
}

/** The node's own pages, over a client of its own. */
function Connected(props: { name: string }): JSX.Element {
  const client = new RpcClient({
    url: `wss://${location.host}/nodes/${props.name}/rpc`,
  });
  const [basic, setBasic] = createSignal<GetBasicStateResponse>();
  const [failure, setFailure] = createSignal<string>();
  // The node may be between restarts: ask again, more slowly each time.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let closed = false;
  const ask = (delayMs: number) => {
    client.call("getBasicState", {}).then(
      (state) => setBasic(state),
      (error: unknown) => {
        if (closed) return;
        setFailure(describe(error));
        timer = setTimeout(() => ask(Math.min(delayMs * 2, 10_000)), delayMs);
      },
    );
  };
  ask(1_000);
  onCleanup(() => {
    closed = true;
    clearTimeout(timer);
    client.close();
  });
  return (
    <RpcContext value={client}>
      <Show
        when={basic()}
        fallback={
          <Show
            when={failure()}
            fallback={<Spinner label={`Connecting to ${props.name}…`} />}
          >
            <Alert title={`Cannot reach ${props.name}`} tone="danger">
              {failure()}. Trying again…
            </Alert>
          </Show>
        }
      >
        {(state) => <WithData name={props.name} basic={state()} />}
      </Show>
    </RpcContext>
  );
}

function WithData(
  props: { name: string; basic: GetBasicStateResponse },
): JSX.Element {
  const data = createNodeData({
    nodeId: props.basic.nodeId,
    ilpAllocationScheme: props.basic.ilpAllocationScheme,
    hostname: props.basic.hostname,
  });
  return (
    <NodeDataContext value={data}>
      <Router base={`/nodes/${props.name}`}>
        <DebugPage />
      </Router>
    </NodeDataContext>
  );
}
