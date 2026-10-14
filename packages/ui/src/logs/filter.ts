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

function haystack(line: LogLine, source: string): string {
  const fields = line.fields.map((field) => `${field.name}=${field.value}`);
  return [source, line.target, line.message, ...fields].join(" ")
    .toLowerCase();
}

/**
 * Whether `line` passes `filter`; its source, where lines have one (the dev
 * dashboard's node names), counts as part of it.
 */
export function matches(line: LogLine, filter: Filter, source = ""): boolean {
  if (filter.include.length === 0 && filter.exclude.length === 0) return true;
  const text = haystack(line, source);
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

/** A piece of a log message: text, or a dev node's vanity ID. */
export type Piece = string | { readonly id: string; readonly name: string };

/**
 * `text` with the dev nodes' vanity IDs (`d12_…`) picked out, so the dev
 * dashboard can show them as links named `d12` (spec 6 §2).
 */
export function splitIds(text: string): Piece[] {
  const pieces: Piece[] = [];
  let last = 0;
  for (const match of text.matchAll(/\b(d[1-9]\d{0,3})_[\w-]{25,30}\b/g)) {
    if (match.index > last) pieces.push(text.slice(last, match.index));
    pieces.push({ id: match[0], name: match[1] });
    last = match.index + match[0].length;
  }
  if (last < text.length) pieces.push(text.slice(last));
  return pieces;
}
