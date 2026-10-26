/**
 * The network (spec/areas/wallet.md#pages): the graph, the node listing by
 * distance, and the selected node's details, with what the owner can do about
 * it. A periphery node is shown as one: with lines to its uplinks, and
 * its uplinks and relay where a core node's registration is.
 */

import type { JSX } from "@solidjs/web";
import {
  type GetStatusResponse,
  type NetworkNode,
  PeerState,
} from "@dassie/rpc/proto";
import {
  type Accessor,
  createMemo,
  createSignal,
  For,
  onCleanup,
  Show,
} from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { currencyOf } from "@dassie/ui/currency";
import { dateTime, shortId } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { NetworkGraph } from "@dassie/ui/network";
import {
  groupNodes,
  peerStateName,
  routingGraph,
  uplinks,
} from "@dassie/ui/network/nodes";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Link, type RouteProps, useRouter } from "@dassie/ui/router";
import { useToast } from "@dassie/ui/toast";
import { Badge, Card, ConfirmButton, LiveNote, PageTitle } from "@dassie/ui/ui";
import { useStatus } from "../../status.ts";
import { feedView } from "./feed.ts";
import {
  NO_UPLINKS,
  relayState,
  uplinkConnected,
  uplinkState,
} from "./periphery.ts";

export function Network(props: RouteProps): JSX.Element {
  const data = useNodeData();
  const router = useRouter();
  const status = useStatus();
  const periphery = () => status()?.periphery === true;
  const nodes = createMemo(() => Object.values(data.nodes.byId));
  const graph = createMemo(() => routingGraph(nodes(), periphery()));
  const selected = () => props.params.node;
  const node = () => (selected() ? data.nodes.byId[selected()!] : undefined);
  return (
    <div class="page-wide stack">
      <PageTitle>
        Network <Show when={selected()}>{shortId(selected()!, 16)}</Show>
      </PageTitle>
      <LiveNote status={data.nodesStatus()} />
      <Card class="graph-card">
        <NetworkGraph
          nodes={graph().nodes}
          links={graph().links}
          selfId={data.selfId}
          selected={selected()}
          hrefFor={(id) => router.href(`/network/${id}`)}
          onSelect={(id) => router.navigate(`/network/${id}`)}
        />
      </Card>
      <div class="grid-2">
        <Listing nodes={nodes()} periphery={periphery()} />
        <Show
          when={node() && node()!.id !== data.selfId}
          fallback={
            // Nothing until the status says which card fits, so that a
            // periphery node never shows a core node's registration.


              <Show when={status()}>
                <Show
                  when={periphery()}
                  fallback={<OwnNode status={status()} />}
                >
                  <PeripheryNode nodes={nodes()} status={status()} />
                </Show>
              </Show>

          }
        >
          <NodeDetail node={node()!} />
        </Show>
      </div>
    </div>
  );
}

function Listing(
  props: { nodes: readonly NetworkNode[]; periphery: boolean },
): JSX.Element {
  return (
    <Card title="Nodes" testid="node-listing">
      <For each={groupNodes(props.nodes)}>
        {(group) => (
          <section class="node-group">
            <h3>
              <span class="distance" aria-hidden="true">{group.mark}</span>{" "}
              {group.heading}
            </h3>
            <ul>
              <For each={group.nodes}>
                {(node) => (
                  <li>
                    <Link
                      href={`/network/${node.id}`}
                      testid={`node-row-${node.id}`}
                    >
                      <strong>{node.alias || shortId(node.id, 20)}</strong>
                    </Link>
                    <div class="muted">
                      {node.url ||
                        (node.isSelf && props.periphery
                          ? "a periphery node, reached through its uplinks"
                          : "no link state")}
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </section>
        )}
      </For>
    </Card>
  );
}

/**
 * This node's standing: registered with the bootstrap nodes or not, and its
 * rate feed.
 */
function OwnNode(
  props: { status: GetStatusResponse | undefined },
): JSX.Element {
  const registration = createLiveValue(
    "subscribeRegistration",
    () => ({}),
    (item) => item,
  );
  return (
    <Card title="Details for Your Node" testid="own-node">
      <h3>Tier 1 Registration</h3>
      <LiveNote status={registration.status()} />
      <Show when={registration.value()}>
        {(status) => (
          <>
            <p>
              <Show
                when={status().member}
                fallback={
                  <Badge tone="bad" testid="registration-badge">
                    Not registered
                  </Badge>
                }
              >
                <Badge tone="good" testid="registration-badge">
                  Registered
                </Badge>
              </Show>
            </p>
            <p class="muted">
              Listed by {status().bootstrap.filter((b) => b.listed).length} of
              {" "}
              {status().bootstrap.length} bootstrap nodes.
            </p>
            <Link
              href="/network/register"
              class="button"
              testid="registration-link"
            >
              {status().member ? "Update registration" : "Register"}
            </Link>
          </>
        )}
      </Show>
      <h3>Rate Feed</h3>
      <Show when={props.status} fallback={<p class="muted">Asking…</p>}>
        {(status) => {
          const feed = () => feedView(status().feed);
          return (
            <div data-testid="rate-feed">
              <p>
                <Badge tone={feed().tone}>{feed().badge}</Badge> {feed().text}
              </p>
              <For each={feed().details}>
                {(detail) => <p class="muted">{detail}</p>}
              </For>
            </div>
          );
        }}
      </Show>
    </Card>
  );
}

/**
 * A periphery node registers with no bootstrap node: it reaches the network
 * through its uplinks, and Open Payments clients reach it through its relay
 * (spec/areas/network.md#periphery-nodes).
 */
function PeripheryNode(props: {
  nodes: readonly NetworkNode[];
  status: GetStatusResponse | undefined;
}): JSX.Element {
  const now = useNow();
  const relay = () => props.status?.relay ?? null;
  return (
    <Card title="Details for Your Node" testid="own-node">
      <h3>Periphery Node</h3>
      <p class="muted">
        Your node has no public address, so it registers with no bootstrap node.
        It reaches the network through its uplinks, core nodes it peers with,
        and is paid through them.
      </p>
      <h3>Uplinks</h3>
      <Show
        when={uplinks(props.nodes).length > 0}
        fallback={<p data-testid="uplinks-none">{NO_UPLINKS}</p>}
      >
        <ul data-testid="uplinks">
          <For each={uplinks(props.nodes)}>
            {(uplink) => (
              <li>
                <Link href={`/network/${uplink.id}`}>
                  <strong>{uplink.alias || shortId(uplink.id, 20)}</strong>
                </Link>{" "}
                <Badge tone={uplinkConnected(uplink.peering) ? "good" : "bad"}>
                  {uplinkState(uplink.peering, now())}
                </Badge>
                <div class="muted">{uplink.url}</div>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <h3>Relay</h3>
      <Show when={props.status} fallback={<p class="muted">Asking…</p>}>
        <p data-testid="relay">
          <Show when={relay()}>
            {(chosen) => <span>{chosen().url}{" "}</span>}
          </Show>
          <Badge tone={relayState(relay()).tone}>
            {relayState(relay()).text}
          </Badge>
        </p>
      </Show>
    </Card>
  );
}

/** How often "last heard from …" is brought up to date. */
const CLOCK_MS = 10_000;

/** The time, ticking every [`CLOCK_MS`]. */
function useNow(): Accessor<number> {
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(timer));
  return now;
}

/** Another node: what the table knows, and the peering with it. */
function NodeDetail(props: { node: NetworkNode }): JSX.Element {
  const client = useClient();
  const data = useNodeData();
  const currency = (scheme: string) => {
    const ledger = data.ledgers()?.find((l) => l.settlementScheme === scheme);
    return ledger ? ledgerCurrency(ledger) : currencyOf("?", 9);
  };
  const toast = useToast();
  const node = () => props.node;
  const peered = () =>
    node().peerState === PeerState.PEERED ||
    node().peerState === PeerState.TERMINATING;
  const act = (title: string, work: () => Promise<unknown>) => async () => {
    try {
      await work();
      toast({ title });
    } catch (failure) {
      toast({
        title: "That did not work",
        description: describe(failure),
        tone: "destructive",
      });
    }
  };
  return (
    <Card
      title={`Details for ${node().alias || shortId(node().id, 20)}`}
      testid="node-detail"
    >
      <dl class="facts">
        <dt>ID</dt>
        <dd>
          <code>{node().id}</code>
        </dd>
        <dt>URL</dt>
        <dd data-testid="node-url">{node().url || "no link state"}</dd>
        <dt>Distance</dt>
        <dd data-testid="node-distance">{node().distance ?? "no route"}</dd>
        <dt>Settles with</dt>
        <dd>{node().settlementSchemes.join(", ") || "—"}</dd>
        <dt>Neighbors</dt>
        <dd>
          {node().neighbors.map((id) => shortId(id, 10)).join(", ") || "—"}
        </dd>
        <dt>Peering</dt>
        <dd data-testid="peer-state">{peerStateName(node().peerState)}</dd>
      </dl>
      <Show when={node().banned}>
        <p data-testid="ban-reason">
          <Badge tone="bad">Banned</Badge> Banned by you.
        </p>
      </Show>
      <Show when={node().peering}>
        {(peering) => (
          <dl class="facts">
            <dt>Scheme</dt>
            <dd>{peering().scheme}</dd>
            <dt>Since</dt>
            <dd>{dateTime(peering().sinceUnixMs)}</dd>
            <dt>Status</dt>
            <dd data-testid="peer-online">
              {peering().online ? "online" : "offline"}
            </dd>
            <dt>Last heard</dt>
            <dd>{dateTime(peering().lastSeenUnixMs)}</dd>
            <dt>Advertised</dt>
            <dd>{peering().advertised ? "yes" : "no"}</dd>
            <dt>Owed to it</dt>
            <dd>
              <Amount
                value={peering().owed}
                currency={currency(peering().scheme)}
              />
            </dd>
            <Show when={peering().terminationReason}>
              <dt>Ending, because</dt>
              <dd>{peering().terminationReason}</dd>
            </Show>
          </dl>
        )}
      </Show>
      <div class="actions">
        <Show when={peered() && node().peerState !== PeerState.TERMINATING}>
          <ConfirmButton
            label="Terminate peering"
            title="End this peering?"
            confirm="Terminate"
            tone="danger"
            testid="terminate-peering"
            onConfirm={act(
              "Ending the peering",
              () => client.call("terminatePeering", { node: node().id }),
            )}
          >
            <p>
              Your node first asks this peer to settle once more, without
              telling it why, and takes no new payments from it. Then it tells
              the peer, and once the payments in flight are done, the peering
              ends. Nothing else is settled: what either side still owes stays
              on the books, to be settled if you peer again. Your node does not
              peer with it again for a day.
            </p>
          </ConfirmButton>
        </Show>
        <Show
          when={node().banned}
          fallback={
            <ConfirmButton
              label="Ban node"
              title="Ban this node?"
              confirm="Ban"
              tone="danger"
              testid="ban-node"
              onConfirm={act(
                "Banned",
                () => client.call("banNode", { node: node().id }),
              )}
            >
              <p>
                Your node ends any peering with it at once: it stops routing
                through it and tells it now, without asking it to settle first.
                What it owes stays on the books. Your node never peers with it
                again until you lift the ban.
              </p>
            </ConfirmButton>
          }
        >
          <button
            type="button"
            data-testid="unban-node"
            onClick={act(
              "Ban lifted",
              () => client.call("unbanNode", { node: node().id }),
            )}
          >
            Unban node
          </button>
        </Show>
      </div>
    </Card>
  );
}
