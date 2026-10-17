/**
 * The network as a graph (spec 6 §1.6.10): the nodes and links it is
 * given, which on the network page are the routing table (spec 7 §5).
 * Drawn in SVG with attributes and classes only, so the page's CSP holds;
 * the layout settles at once for readers who prefer less motion.
 */

import type { JSX } from "@solidjs/web";
import type { NetworkNode } from "@dassie/rpc/proto";
import { createEffect, createSignal, For, onCleanup } from "solid-js";
import { shortId } from "../format.ts";
import { Layout } from "./layout.ts";
import { type Link, seedIndex } from "./nodes.ts";

export function NetworkGraph(props: {
  nodes: readonly NetworkNode[];
  links: readonly Link[];
  selfId: string;
  selected?: string;
  onSelect?: (id: string) => void;
  hrefFor: (id: string) => string;
  /** Links with traffic just now, drawn highlighted (the dev dashboard). */
  active?: (link: Link) => boolean;
}): JSX.Element {
  const layout = new Layout();
  const [frame, setFrame] = createSignal(0);
  const still = typeof matchMedia === "function" &&
    matchMedia("(prefers-reduced-motion: reduce)").matches;
  let request: number | undefined;
  const step = () => {
    layout.tick(props.links);
    setFrame((n) => n + 1);
    request = layout.settled ? undefined : requestAnimationFrame(step);
  };
  createEffect(
    () => ({ ids: props.nodes.map((node) => node.id), links: props.links }),
    ({ ids, links }) => {
      layout.update(ids);
      if (still) {
        layout.settle(links);
        setFrame((n) => n + 1);
      } else if (request === undefined) {
        request = requestAnimationFrame(step);
      }
    },
  );
  onCleanup(() => {
    if (request !== undefined) cancelAnimationFrame(request);
  });
  const at = (id: string) => {
    frame();
    return layout.bodies.get(id) ?? { x: 0, y: 0 };
  };
  const box = () => {
    frame();
    const b = layout.bounds();
    return `${b.x} ${b.y} ${b.width} ${b.height}`;
  };
  const label = (node: NetworkNode) =>
    node.alias ||
    (node.id === props.selfId ? "this node" : shortId(node.id, 8));
  return (
    <svg
      class="network-graph"
      data-testid="network-graph"
      viewBox={box()}
      role="img"
      aria-label={`${props.nodes.length} nodes and ${props.links.length} links`}
    >
      <For each={props.links}>
        {(link) => (
          <line
            class={props.active?.(link) ? "graph-link active" : "graph-link"}
            data-testid="graph-link"
            x1={at(link.a).x}
            y1={at(link.a).y}
            x2={at(link.b).x}
            y2={at(link.b).y}
          />
        )}
      </For>
      <For each={props.nodes}>
        {(node) => (
          // A link inside SVG would be an HTML element to the compiler, and
          // not drawn: a focusable group stands in for it.
          <g
            class="graph-node-group"
            role="link"
            tabindex="0"
            data-testid="graph-node"
            data-href={props.hrefFor(node.id)}
            aria-label={node.alias || node.id}
            transform={`translate(${at(node.id).x} ${at(node.id).y})`}
            onClick={() => props.onSelect?.(node.id)}
            onKeyDown={(event: KeyboardEvent) => {
              if (event.key === "Enter") props.onSelect?.(node.id);
            }}
          >
            <circle
              r={node.id === props.selfId ? 9 : 7}
              class={`graph-node seed-${seedIndex(node.id)} ${
                node.id === props.selected ? "selected" : ""
              }`}
            />
            <text class="graph-label" y="20">{label(node)}</text>
          </g>
        )}
      </For>
    </svg>
  );
}
