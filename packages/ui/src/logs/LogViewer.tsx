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
import { matches, parseFilter, visibleRange } from "./filter.ts";

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

export function LogViewer(props: {
  lines: Accessor<readonly LogLine[]>;
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
    return props.lines().filter((line) =>
      line.level >= level && matches(line, filter)
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
          onChange={(event) => setLowest(Number(event.currentTarget.value))}
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
              {(line) => <Row line={line} />}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
}

function Row(props: { line: LogLine }): JSX.Element {
  const fields = () =>
    props.line.fields.map((field) => `${field.name}=${field.value}`).join(" ");
  return (
    <div class="log-row" data-testid="log-row" title={fields()}>
      <span class="log-time">{clockTime(props.line.timeUnixMs)}</span>
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
        {props.line.message} <span class="log-fields">{fields()}</span>
      </span>
    </div>
  );
}
