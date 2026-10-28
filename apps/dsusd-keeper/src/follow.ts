// When the keeper runs its plan while it follows the pool
// (spec/areas/dsusd-keeper.md#following-the-pool): at once after someone
// else's transaction touched the pool, and otherwise once a minute, for
// the market's moves; with a budget per window of time, not per run; and
// for a bounded life, after which systemd starts it again from the newest
// `main`. The clock is passed in, so the tests move it.

import { BUDGET, type Budget } from "./plan.ts";

/** A window of time with a budget of its own: what one run had when the
 * keeper ran every five minutes, so following spends no more than that
 * did. */
export const WINDOW_MS = 5 * 60_000;
/** How many windows a process lives: an hour. */
export const WINDOWS_PER_LIFE = 12;
/** How long the keeper goes without a run before it runs for the
 * market's moves alone. */
export const TICK_MS = 60_000;

/** When runs are due, and what each window may still spend. */
export class Schedule {
  readonly #startMs: number;
  #window = 0;
  #budget: Budget = BUDGET;
  /** A run is due: the first, to catch up, and one after each pool event
   * that no run has read yet. */
  #due = true;
  #lastStartMs: number;
  /** The newest ledger a pool event came in, and the newest one a run
   * started after, so read: an event of that ledger or an older one asks
   * for no run. */
  #seenThrough = 0;
  #readThrough = 0;

  constructor(startMs: number) {
    this.#startMs = startMs;
    this.#lastStartMs = startMs;
  }

  /** Someone else's transaction touched the pool in validated ledger
   * `ledgerIndex`. */
  poolTouched(ledgerIndex: number): void {
    if (ledgerIndex > this.#readThrough) this.#due = true;
    if (ledgerIndex > this.#seenThrough) this.#seenThrough = ledgerIndex;
  }

  /** The keeper connected again: what touched the pool meanwhile went
   * unseen, so a run is due. */
  catchUp(): void {
    this.#due = true;
  }

  /** Starts a run if one is due at `nowMs`: the window it starts in, and
   * what that window may still spend. */
  start(nowMs: number): { window: number; budget: Budget } | null {
    if (!this.#due && nowMs - this.#lastStartMs < TICK_MS) return null;
    const window = this.windowOf(nowMs);
    if (window !== this.#window) {
      this.#window = window;
      this.#budget = BUDGET;
    }
    this.#due = false;
    this.#lastStartMs = nowMs;
    // The run reads the newest validated ledger, which holds every pool
    // event seen so far.
    this.#readThrough = this.#seenThrough;
    return { window, budget: this.#budget };
  }

  /** A run that started in `window` left `budget`. A run that outlasted
   * its window spent that window's budget, never the next one's. */
  finish(window: number, budget: Budget): void {
    if (window === this.#window) this.#budget = budget;
  }

  /** When the next run is due without a pool event. */
  tickAt(): number {
    return this.#lastStartMs + TICK_MS;
  }

  /** The window `nowMs` is in, counted from the process's start. */
  windowOf(nowMs: number): number {
    const elapsed = nowMs > this.#startMs ? nowMs - this.#startMs : 0;
    return Math.floor(elapsed / WINDOW_MS);
  }

  /** When the process ends: after its life, or, once it failed, at the
   * end of the window it failed in. It never ends within a window, so the
   * next process's first window never shares time with one of this one's,
   * and a keeper that fails again and again spends no more for it. */
  endAt(nowMs: number, failed: boolean): number {
    const windows = failed ? this.windowOf(nowMs) + 1 : WINDOWS_PER_LIFE;
    return this.#startMs + windows * WINDOW_MS;
  }
}

/** The validated ledger of a transaction stream message about the pool,
 * if a transaction of an account not in `own` sent it; the keeper's and
 * the issuer's own leave the pool where their run put it. Takes both API
 * versions' fields: `tx_json` in version 2, `transaction` in 1. */
export function poolEvent(
  message: unknown,
  own: ReadonlySet<string>,
): number | null {
  if (typeof message !== "object" || message === null) return null;
  const fields = message as Record<string, unknown>;
  if (fields["type"] !== "transaction" || fields["validated"] !== true) {
    return null;
  }
  const ledgerIndex = fields["ledger_index"];
  if (
    typeof ledgerIndex !== "number" || !Number.isSafeInteger(ledgerIndex) ||
    ledgerIndex <= 0
  ) {
    return null;
  }
  const tx = fields["tx_json"] ?? fields["transaction"];
  const account = typeof tx === "object" && tx !== null
    ? (tx as Record<string, unknown>)["Account"]
    : undefined;
  if (typeof account === "string" && own.has(account)) return null;
  return ledgerIndex;
}
