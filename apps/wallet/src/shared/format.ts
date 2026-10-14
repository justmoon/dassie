/** Small formatters the pages share. */

/** A time of day with milliseconds, for log lines. */
export function clockTime(unixMs: bigint | number): string {
  const date = new Date(Number(unixMs));
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${
    pad(date.getSeconds())
  }.${pad(date.getMilliseconds(), 3)}`;
}

/** How long ago, or until, `unixMs` is from `nowMs`, in words. */
export function relativeTime(unixMs: bigint | number, nowMs: number): string {
  const at = Number(unixMs);
  if (at === 0) return "never";
  const seconds = Math.round((nowMs - at) / 1000);
  const magnitude = Math.abs(seconds);
  const [value, unit] = magnitude < 60
    ? [magnitude, "s"]
    : magnitude < 3600
    ? [Math.round(magnitude / 60), "min"]
    : magnitude < 86_400
    ? [Math.round(magnitude / 3600), "h"]
    : [Math.round(magnitude / 86_400), "d"];
  return seconds >= 0 ? `${value} ${unit} ago` : `in ${value} ${unit}`;
}

/** A date and time for people. */
export function dateTime(unixMs: bigint | number): string {
  return Number(unixMs) === 0 ? "—" : new Date(Number(unixMs)).toLocaleString();
}

/** Bytes as hex. */
export function hex(bytes: Uint8Array): string {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** A node ID cut for tight spaces: its first characters and an ellipsis. */
export function shortId(id: string, length = 12): string {
  return id.length > length ? `${id.slice(0, length)}…` : id;
}

/** JSON that also takes bigints and bytes. */
export function prettyJson(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item) =>
      typeof item === "bigint"
        ? item.toString()
        : item instanceof Uint8Array
        ? hex(item)
        : item,
    2,
  );
}
