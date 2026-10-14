/**
 * The network (spec 6 §1.6.10): the graph, the node listing by distance,
 * and the selected node's details, with what the owner can do about it
 * (UI-4).
 */

import type { JSX } from "@solidjs/web";
import { type NetworkNode, PeerState } from "@dassie/rpc/proto";
import { createMemo, For, Show } from "solid-js";
import { Amount, ledgerCurrency } from "@dassie/ui/amount";
import { currencyOf } from "@dassie/ui/currency";
import { dateTime, shortId } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { NetworkGraph } from "@dassie/ui/network";
import {
  graphLinks,
  groupNodes,
  peerStateName,
} from "@dassie/ui/network/nodes";
import { useNodeData } from "@dassie/ui/node-data";
import { describe, useClient } from "@dassie/ui/rpc";
import { Link, type RouteProps, useRouter } from "@dassie/ui/router";
import { useToast } from "@dassie/ui/toast";
import { Badge, Card, ConfirmButton, LiveNote, PageTitle } from "@dassie/ui/ui";

export function Network(props: RouteProps): JSX.Element {
  const data = useNodeData();
  const router = useRouter();
  const nodes = createMemo(() => Object.values(data.nodes.byId));
  const links = createMemo(() => graphLinks(nodes(), data.selfId));
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
          nodes={nodes()}
          links={links()}
          selfId={data.selfId}
          selected={selected()}
          hrefFor={(id) => router.href(`/network/${id}`)}
          onSelect={(id) => router.navigate(`/network/${id}`)}
        />
      </Card>
      <div class="grid-2">
        <Listing nodes={nodes()} />
        <Show
          when={node() && node()!.id !== data.selfId}
          fallback={<OwnNode />}
        >
          <NodeDetail node={node()!} />
        </Show>
      </div>
    </div>
  );
}

function Listing(props: { nodes: readonly NetworkNode[] }): JSX.Element {
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
                    <div class="muted">{node.url || "no link state"}</div>
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

/** This node's standing: registered with the bootstrap nodes or not. */
function OwnNode(): JSX.Element {
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
    </Card>
  );
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
        <dd>{node().distance ?? "???"}</dd>
        <dt>Settles with</dt>
        <dd>{node().settlementSchemes.join(", ") || "—"}</dd>
        <dt>Neighbors</dt>
        <dd>
          {node().neighbors.map((id) => shortId(id, 10)).join(", ") || "—"}
        </dd>
        <dt>Peering</dt>
        <dd data-testid="peer-state">{peerStateName(node().peerState)}</dd>
      </dl>
      <Show when={node().peering}>
        {(peering) => (
          <dl class="facts">
            <dt>Scheme</dt>
            <dd>{peering().scheme}</dd>
            <dt>Since</dt>
            <dd>{dateTime(peering().sinceUnixMs)}</dd>
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
              No new payments go through this peer. Once those in flight are
              done, the peering ends and what is owed is settled. Your node does
              not peer with it again for a day.
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
                Your node ends any peering with it, and never peers with it
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
