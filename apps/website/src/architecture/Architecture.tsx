/**
 * The architecture map: Dassie's crates, apps and packages as a layered
 * diagram with a detail panel, journeys through it, how it is tested, and
 * the releases. Everything comes from `map.json`. Colours are classes,
 * never style attributes, for the site's CSP.
 */

import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
  type ArchitectureMap,
  BOX_HEIGHT,
  type EdgeKind,
  edgePath,
  flow,
  highlight,
  type Selection,
  sourceUrl,
  subsystem,
  subsystemClass,
} from "./map.ts";

const KINDS: EdgeKind[] = ["uses", "talks", "runs", "wasm"];
const KIND_OUT: Record<EdgeKind, string> = {
  uses: "Builds on",
  talks: "Talks over",
  runs: "Runs",
  wasm: "Compiles to WebAssembly from",
};
const KIND_IN: Record<EdgeKind, string> = {
  uses: "Used by",
  talks: "Spoken by",
  runs: "Run by",
  wasm: "Compiled into",
};
const KIND_LEGEND: Record<EdgeKind, string> = {
  uses: "builds on",
  talks: "talks over (a contract)",
  runs: "runs the binary",
  wasm: "compiled to WebAssembly from",
};

const date = (day: string) =>
  new Date(`${day}T12:00:00Z`).toLocaleDateString("en", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  });

export function Architecture(props: { map: ArchitectureMap }): JSX.Element {
  const map = props.map;
  const [selection, setSelection] = createSignal<Selection>({ type: "none" });
  const [showAll, setShowAll] = createSignal(false);
  const lit = createMemo(() => highlight(map, selection()));
  const journey = () => {
    const now = selection();
    return now.type === "flow" ? now.id : undefined;
  };
  /** Follows a journey, or stops following it when it is followed. */
  const follow = (id: string) =>
    setSelection(journey() === id ? { type: "none" } : { type: "flow", id });
  const pick = (id: string) => setSelection({ type: "component", id });

  return (
    <>
      <div class="intro">
        <For each={map.intro}>{(paragraph) => <p>{paragraph}</p>}</For>
        <p>
          Select a box to see what it does and what it connects to, or follow a
          journey through it.
        </p>
      </div>

      <div class="toolbar">
        <span class="label">Follow a journey:</span>
        <For each={map.flows}>
          {(item) => (
            <button
              type="button"
              class="journey"
              data-testid="journey"
              aria-pressed={journey() === item.id ? "true" : "false"}
              onClick={() => follow(item.id)}
            >
              {item.button}
            </button>
          )}
        </For>
        <button
          type="button"
          class="clear"
          onClick={() => setSelection({ type: "none" })}
        >
          Clear
        </button>
        <label class="every">
          <input
            type="checkbox"
            checked={showAll()}
            onChange={(event) => setShowAll(event.currentTarget.checked)}
          />{" "}
          Show every link
        </label>
      </div>

      <div class="stage">
        <div class="mapbox">
          <Diagram
            map={map}
            selection={selection}
            lit={lit}
            showAll={showAll}
            pick={pick}
          />
        </div>
        <aside class="panel" aria-live="polite" data-testid="panel">
          <Panel map={map} selection={selection} pick={pick} />
        </aside>
      </div>

      <section class="more">
        <h2>How it is tested</h2>
        <p class="lead">{map.testsIntro}</p>
        <div class="tests">
          <For each={map.tests}>
            {(kind) => (
              <div class="tcard">
                <h3>{kind.t}</h3>
                <p>
                  <Prose text={kind.p} />
                </p>
                <p class="ev">{kind.ev}</p>
              </div>
            )}
          </For>
        </div>
      </section>

      <section class="more">
        <h2>Releases</h2>
        <div class="releases">
          <For each={map.releases}>
            {(release) => (
              <article class="release" data-testid="release">
                <h3>
                  {release.version}
                  <time datetime={release.date}>{date(release.date)}</time>
                </h3>
                <ul>
                  <For each={release.changes}>
                    {(change) => (
                      <li>
                        <Prose text={change} />
                      </li>
                    )}
                  </For>
                </ul>
              </article>
            )}
          </For>
        </div>
      </section>

      <p class="foot">
        The{" "}
        <a href="https://github.com/justmoon/dassie/blob/main/spec/README.md">
          specification
        </a>{" "}
        describes the design in depth, and its{" "}
        <a href="https://github.com/justmoon/dassie/blob/main/spec/decisions/README.md">
          decisions
        </a>{" "}
        the reasons behind it.
      </p>
    </>
  );
}

/** Text in which `backticks` mark code, such as a command. */
function Prose(props: { text: string }): JSX.Element {
  return (
    <For each={props.text.split("`")}>
      {(part, index) => index() % 2 === 1 ? <code>{part}</code> : part}
    </For>
  );
}

function Diagram(props: {
  map: ArchitectureMap;
  selection: () => Selection;
  lit: () => ReturnType<typeof highlight>;
  showAll: () => boolean;
  pick: (id: string) => void;
}): JSX.Element {
  const map = props.map;
  const C = map.components;
  const edgeClass = (index: number) => {
    const edge = map.edges[index];
    const { boxes, edges } = props.lit();
    const on = edges.has(index);
    const off = edge.h && !props.showAll() && !on;
    const dim = boxes !== null && !on && !off;
    return `edge ${edge.k}${on ? " hi" : ""}${off ? " off" : ""}${
      dim ? " dim" : ""
    }`;
  };
  const marker = (id: string, cls: string) => (
    <marker
      id={id}
      viewBox="0 0 10 10"
      refX="9"
      refY="5"
      markerWidth="7"
      markerHeight="7"
      orient="auto-start-reverse"
    >
      <path d="M0,1 L9,5 L0,9 z" class={cls} />
    </marker>
  );
  return (
    <svg
      viewBox={`0 0 ${map.viewBox.w} ${map.viewBox.h}`}
      role="img"
      aria-label="Dassie's parts in layers, from the browser at the top to the protocol cores at the bottom"
    >
      <defs>
        {marker("arr", "arrow")}
        {marker("arrhi", "arrow-hi")}
      </defs>
      <For each={map.bands}>
        {(band) => (
          <rect
            x={band.x}
            y={band.y}
            width={band.w}
            height={band.h}
            rx="10"
            class="band"
          />
        )}
      </For>
      <g>
        <For each={map.edges}>
          {(edge, index) => (
            <path
              d={edgePath(C[edge.f], C[edge.t])}
              class={edgeClass(index())}
              marker-end={props.lit().edges.has(index())
                ? "url(#arrhi)"
                : "url(#arr)"}
            />
          )}
        </For>
      </g>
      {/* Over the links, so a title's halo hides those it crosses. */}
      <For each={map.bands}>
        {(band) => (
          <text x={band.x + 14} y={band.y + 26} class="bandt">
            <tspan class="bandl">{band.t}</tspan>
            <tspan class="bands" dx="12">{band.s}</tspan>
          </text>
        )}
      </For>
      <For each={Object.entries(C)}>
        {([id, box]) => {
          const h = box.h ?? BOX_HEIGHT;
          const badge = () => props.lit().badges.get(id);
          const state = () => {
            const { boxes } = props.lit();
            const now = props.selection();
            return `${boxes !== null && !boxes.has(id) ? " dim" : ""}${
              now.type === "component" && now.id === id ? " sel" : ""
            }`;
          };
          const select = () => props.pick(id);
          return (
            <g
              class={`box ${subsystemClass(box.sub)}${state()}`}
              tabindex="0"
              role="button"
              aria-label={`${box.name}: ${box.label}`}
              data-testid="box"
              data-id={id}
              onClick={select}
              onKeyDown={(event: KeyboardEvent) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  select();
                }
              }}
            >
              <rect
                x={box.x}
                y={box.y}
                width={box.w}
                height={h}
                rx="8"
                class="bx"
              />
              <text x={box.x + 12} y={box.y + h / 2 - 4} class="nm">
                {box.name}
              </text>
              <text x={box.x + 12} y={box.y + h / 2 + 17} class="lb">
                {box.label}
              </text>
              <Show when={badge()}>
                {(step) => (
                  <g class="step">
                    <circle cx={box.x + 2} cy={box.y + 2} r="12" />
                    <text x={box.x + 2} y={box.y + 7} text-anchor="middle">
                      {step()}
                    </text>
                  </g>
                )}
              </Show>
            </g>
          );
        }}
      </For>
      <For each={map.notes}>
        {(note) => (
          <text
            x={note.x}
            y={note.y}
            text-anchor={note.anchor ?? "start"}
            class="note"
          >
            {note.text}
          </text>
        )}
      </For>
    </svg>
  );
}

function Chip(
  props: { map: ArchitectureMap; id: string; pick: (id: string) => void },
): JSX.Element {
  const box = () => props.map.components[props.id];
  return (
    <button
      type="button"
      class={`chipbtn ${subsystemClass(box()?.sub ?? "")}`}
      onClick={() => props.pick(props.id)}
    >
      {box()?.name ?? props.id}
    </button>
  );
}

function Legend(props: { map: ArchitectureMap }): JSX.Element {
  return (
    <>
      <ul class="legend">
        <For each={props.map.subsystems}>
          {(sub) => (
            <li>
              <span class={`sw ${subsystemClass(sub.id)}`} />
              <span>
                <b>{sub.name}</b>: {sub.desc}
              </span>
            </li>
          )}
        </For>
      </ul>
      <ul class="legend lines">
        <For each={KINDS}>
          {(kind) => (
            <li>
              <svg width="40" height="12" aria-hidden="true">
                <path d="M1,6 H39" class={`edge ${kind}`} />
              </svg>
              <span>{KIND_LEGEND[kind]}</span>
            </li>
          )}
        </For>
      </ul>
    </>
  );
}

function Panel(props: {
  map: ArchitectureMap;
  selection: () => Selection;
  pick: (id: string) => void;
}): JSX.Element {
  const map = props.map;
  const chips = (ids: string[]) => (
    <div class="chips">
      <For each={ids}>
        {(id) => <Chip map={map} id={id} pick={props.pick} />}
      </For>
    </div>
  );
  const content = () => {
    const selection = props.selection();
    if (selection.type === "component") {
      const box = map.components[selection.id];
      const out = new Map<EdgeKind, string[]>();
      const into = new Map<EdgeKind, string[]>();
      for (const edge of map.edges) {
        if (edge.f === selection.id) {
          out.set(edge.k, [...out.get(edge.k) ?? [], edge.t]);
        }
        if (edge.t === selection.id) {
          into.set(edge.k, [...into.get(edge.k) ?? [], edge.f]);
        }
      }
      const links = [
        ...KINDS.filter((kind) => out.has(kind)).map((kind) =>
          [KIND_OUT[kind], out.get(kind)!] as const
        ),
        ...KINDS.filter((kind) => into.has(kind)).map((kind) =>
          [KIND_IN[kind], into.get(kind)!] as const
        ),
      ];
      return (
        <>
          <div>
            <p class={`tag ${subsystemClass(box.sub)}`}>
              {subsystem(map, box.sub)?.name}
            </p>
            <h2 class="pname" data-testid="panel-title">{box.name}</h2>
            <p class="muted">{box.label}</p>
          </div>
          <p>
            <Prose text={box.desc} />
          </p>
          <div>
            <h3>What it does</h3>
            <ul>
              <For each={box.does}>
                {(line) => (
                  <li>
                    <Prose text={line} />
                  </li>
                )}
              </For>
            </ul>
          </div>
          <For each={links}>
            {([heading, ids]) => (
              <div>
                <h3>{heading}</h3>
                {chips(ids)}
              </div>
            )}
          </For>
          <p>
            <a href={sourceUrl(box)}>
              Source: <code>{box.path}</code>
            </a>
          </p>
        </>
      );
    }
    if (selection.type === "flow") {
      const item = flow(map, selection.id)!;
      return (
        <>
          <h2 data-testid="panel-title">{item.title}</h2>
          <ol>
            <For each={item.steps}>
              {(step) => (
                <li>
                  <div>
                    {chips(step.ids)}
                    <p>
                      <Prose text={step.text} />
                    </p>
                  </div>
                </li>
              )}
            </For>
          </ol>
          <p class="muted">
            The numbers on the map mark where each step happens.
          </p>
        </>
      );
    }
    return (
      <>
        <h2 data-testid="panel-title">How to read the map</h2>
        <p>
          Each box is a crate, an app, a package or a contract. Layers run top
          to bottom, from what people see to what everything rests on, and an
          arrow points at what a box builds on. Colours group the parts that
          work together.
        </p>
        <Legend map={map} />
        <p>
          Some links are hidden to keep the map readable: dassie-node, for one,
          uses every core crate. Select a box to see all of its links, or turn
          on <i>Show every link</i>.
        </p>
        <div>
          <h3>Try</h3>
          {chips(
            ["node", "connector", "stream", "wallet"].filter((id) =>
              id in map.components
            ),
          )}
        </div>
      </>
    );
  };
  return <>{content()}</>;
}
