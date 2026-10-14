/**
 * The log viewer's filter (spec 6 §1.7): keywords separated by spaces,
 * each of which must match; a keyword starting with `-` must not. A
 * keyword matches a line whose target, message or a field contains it,
 * ignoring case.
 */

import type { LogLine } from "@dassie/rpc/proto";

/** A parsed filter. */
export interface Filter {
  include: string[];
  exclude: string[];
}

/** Parses the filter's text. */
export function parseFilter(text: string): Filter {
  const include: string[] = [];
  const exclude: string[] = [];
  for (const word of text.toLowerCase().split(/\s+/)) {
    if (word === "" || word === "-") continue;
    if (word.startsWith("-")) exclude.push(word.slice(1));
    else include.push(word);
  }
  return { include, exclude };
}

function haystack(line: LogLine): string {
  const fields = line.fields.map((field) => `${field.name}=${field.value}`);
  return [line.target, line.message, ...fields].join(" ").toLowerCase();
}

/** Whether `line` passes `filter`. */
export function matches(line: LogLine, filter: Filter): boolean {
  if (filter.include.length === 0 && filter.exclude.length === 0) return true;
  const text = haystack(line);
  return filter.include.every((word) => text.includes(word)) &&
    !filter.exclude.some((word) => text.includes(word));
}

/** Which rows of a list of `count` rows of `rowHeight` px to render. */
export function visibleRange(
  scrollTop: number,
  viewport: number,
  rowHeight: number,
  count: number,
  overscan = 10,
): { start: number; end: number } {
  const first = Math.floor(scrollTop / rowHeight);
  const visible = Math.ceil(viewport / rowHeight);
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, first + visible + overscan);
  return { start, end: Math.max(start, end) };
}
