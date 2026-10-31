/** The time, for the network pages' "last heard from …" and "in …". */

import { type Accessor, createSignal, onCleanup } from "solid-js";

/** How often the time is brought up to date. */
const CLOCK_MS = 10_000;

/** The time, ticking every [`CLOCK_MS`]. */
export function useNow(): Accessor<number> {
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(timer));
  return now;
}
