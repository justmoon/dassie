/**
 * The STREAM toy's settings form: its fields as typed, and how they become
 * the settings of a run.
 *
 * @module
 */

import {
  checkSettings,
  DEFAULT_SETTINGS,
  type Settings,
} from "./simulation.ts";

/** The fields, as text. */
export interface Form {
  amount: string;
  maxPacketAmount: string;
  latency: string;
  jitter: string;
  maxPacketsInFlight: string;
  seed: string;
  /** How fast the run plays: 1 is real time. */
  timeDilation: string;
}

/** The original's defaults. */
export const DEFAULT_FORM: Form = {
  amount: String(DEFAULT_SETTINGS.amount),
  maxPacketAmount: String(DEFAULT_SETTINGS.maxPacketAmount),
  latency: String(DEFAULT_SETTINGS.latency),
  jitter: String(DEFAULT_SETTINGS.jitter),
  maxPacketsInFlight: String(DEFAULT_SETTINGS.maxPacketsInFlight),
  seed: String(DEFAULT_SETTINGS.seed),
  timeDilation: "1",
};

/** The slowest and fastest playback. */
export const MIN_DILATION = 0.01;
export const MAX_DILATION = 100;

/** A run's settings and how fast it plays. */
export interface Parsed {
  settings: Settings;
  timeDilation: number;
}

function integer(text: string, name: string): bigint | string {
  const trimmed = text.trim();
  if (!/^\d{1,20}$/.test(trimmed)) return `${name} must be a whole number.`;
  return BigInt(trimmed);
}

function number(text: string, name: string): number | string {
  const trimmed = text.trim();
  const value = Number(trimmed);
  if (trimmed === "" || !Number.isFinite(value)) {
    return `${name} must be a number.`;
  }
  return value;
}

/** The settings the form describes, or why it describes none. */
export function parseForm(form: Form): Parsed | string {
  const amount = integer(form.amount, "The amount");
  if (typeof amount === "string") return amount;
  const maxPacketAmount = integer(form.maxPacketAmount, "The largest packet");
  if (typeof maxPacketAmount === "string") return maxPacketAmount;
  const latency = number(form.latency, "The latency");
  if (typeof latency === "string") return latency;
  const jitter = number(form.jitter, "The jitter");
  if (typeof jitter === "string") return jitter;
  const inFlight = number(form.maxPacketsInFlight, "Packets in flight");
  if (typeof inFlight === "string") return inFlight;
  const seed = number(form.seed, "The seed");
  if (typeof seed === "string") return seed;
  const timeDilation = number(form.timeDilation, "The time dilation");
  if (typeof timeDilation === "string") return timeDilation;
  if (timeDilation < MIN_DILATION || timeDilation > MAX_DILATION) {
    return `The time dilation must be between ${MIN_DILATION} and ${MAX_DILATION}.`;
  }
  const settings: Settings = {
    amount,
    maxPacketAmount,
    latency,
    jitter,
    maxPacketsInFlight: inFlight,
    seed,
  };
  return checkSettings(settings) ?? { settings, timeDilation };
}
