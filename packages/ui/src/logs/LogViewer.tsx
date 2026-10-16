/**
 * The log viewer (spec 6 §1.7): a filter, the lowest level shown, and the
 * lines, of which only those in view are rendered. It follows the newest
 * line unless the reader scrolled up.
 */

import type { JSX } from "@solidjs/web";
import { LogLevel, type LogLine } from "@dassie/rpc/proto";
import {
  type Accessor,
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  For,
} from "solid-js";
import { clockTime } from "../format.ts";
import { seedIndex } from "../network/nodes.ts";
import { Link } from "../router/Router.tsx";
import { matches, parseFilter, splitIds, visibleRange } from "./filter.ts";

/** Each row's height, in px; the CSS gives rows the same. */
const ROW_PX = 20;

const LEVELS = [
  [LogLevel.DEBUG, "debug"],
  [LogLevel.INFO, "info"],
  [LogLevel.WARN, "warn"],
  [LogLevel.ERROR, "error"],
] as const;

const levelName = (level: LogLevel) =>
  LEVELS.find(([value]) => value === level)?.[1] ?? "info";

/** Where the dev dashboard's lines come from, and where to link. */
export interface LogSources {
  /** The source of a line: a node's name, or "host". */
  of: (line: LogLine) => string;
  /** The page of a source, or of a node named in a message. */
  href: (source: string) => string;
}

export function LogViewer(props: {
  lines: Accessor<readonly LogLine[]>;
  /** A column naming each line's source, for the dev dashboard. */
  sources?: LogSources;
  testid?: string;
}): JSX.Element {
  const id = createUniqueId();
  const [text, setText] = createSignal("");
  const [lowest, setLowest] = createSignal<LogLevel>(LogLevel.DEBUG);
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(480);
  let viewport: HTMLDivElement | undefined;
  let following = true;
  const shown = createMemo(() => {
    const filter = parseFilter(text());
    const level = lowest();
    const source = props.sources?.of;
    return props.lines().filter((line) =>
      line.level >= level && matches(line, filter, source?.(line))
    );
  });
  const range = createMemo(() =>
    visibleRange(scrollTop(), height(), ROW_PX, shown().length)
  );
  // After the rows are laid out, the newest line comes into view, unless
  // the reader scrolled up.
  createEffect(() => shown().length, () => {
    requestAnimationFrame(() => {
      if (!following || !viewport) return;
      viewport.scrollTop = viewport.scrollHeight;
      setScrollTop(viewport.scrollTop);
      setHeight(viewport.clientHeight);
    });
  });
  const onScroll = () => {
    if (!viewport) return;
    setScrollTop(viewport.scrollTop);
    setHeight(viewport.clientHeight);
    following = viewport.scrollTop + viewport.clientHeight >=
      viewport.scrollHeight - ROW_PX;
  };
  return (
    <div class="log-viewer" data-testid={props.testid ?? "log-viewer"}>
      <div class="log-controls">
        <label for={`${id}-filter`}>Filter</label>
        <input
          id={`${id}-filter`}
          type="search"
          placeholder="keyword -excluded"
          data-testid="log-filter"
          value={text()}
          onInput={(event) => setText(event.currentTarget.value)}
        />
        <label for={`${id}-level`}>Level</label>
        <select
          id={`${id}-level`}
          data-testid="log-level-filter"
          onChange={(event) =>
            setLowest(Number(event.currentTarget.value) as LogLevel)}
        >
          <For each={LEVELS}>
            {([value, name]) => (
              <option value={value} selected={value === lowest()}>
                {name}
              </option>
            )}
          </For>
        </select>
        <span class="muted">{shown().length} lines</span>
      </div>
      <div
        class="log-viewport"
        ref={viewport}
        tabindex="0"
        role="log"
        aria-label="Log lines"
        onScroll={onScroll}
      >
        <div
          class="log-spacer"
          style={{ height: `${shown().length * ROW_PX}px` }}
        >
          <div
            class="log-rows"
            style={{ transform: `translateY(${range().start * ROW_PX}px)` }}
          >
            <For each={shown().slice(range().start, range().end)}>
              {(line) => <Row line={line} sources={props.sources} />}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
}

function Row(props: { line: LogLine; sources?: LogSources }): JSX.Element {
  const fields = () =>
    props.line.fields.map((field) => `${field.name}=${field.value}`).join(" ");
  const source = () => props.sources?.of(props.line);
  return (
    <div class="log-row" data-testid="log-row" title={fields()}>
      <span class="log-time">{clockTime(props.line.timeUnixMs)}</span>
      {props.sources && (
        <span
          class={`log-source seed-${seedIndex(source() ?? "")}`}
          data-testid="log-source"
        >
          <Link href={props.sources.href(source() ?? "")}>
            {source() ?? ""}
          </Link>
        </span>
      )}
      <span
        class={`log-level log-${levelName(props.line.level)}`}
        data-testid="log-level"
      >
        {levelName(props.line.level)}
      </span>
      <span class={`log-target seed-${seedIndex(props.line.target)}`}>
        {props.line.target}
      </span>
      <span class="log-message">
        <Text text={props.line.message} sources={props.sources} />{" "}
        <span class="log-fields">
          <Text text={fields()} sources={props.sources} />
        </span>
      </span>
    </div>
  );
}

/** Text, with dev nodes' IDs as links where the lines have sources. */
function Text(props: { text: string; sources?: LogSources }): JSX.Element {
  return (
    <>
      {props.sources
        ? (
          <For each={splitIds(props.text)}>
            {(piece) =>
              typeof piece === "string"
                ? piece
                : (
                  <Link href={props.sources!.href(piece.name)}>
                    <span title={piece.id}>{piece.name}</span>
                  </Link>
                )}
          </For>
        )
        : props.text}
    </>
  );
}
