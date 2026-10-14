/**
 * The server name a TLS client asks for (SNI), read from its ClientHello,
 * so that the dev proxy can pass the connection on without terminating
 * TLS (spec 6 §6.3). The ClientHello may arrive in pieces, so the parser
 * says when it needs more; it never reads past one TLS record.
 *
 * @module
 */

/** A TLS record's header: type, version, length. */
const RECORD_HEADER_BYTES = 5;
/** The largest plaintext record TLS allows. */
export const MAX_RECORD_BYTES = 16_384;
/** The most the proxy reads before it decides. */
export const MAX_HELLO_BYTES = RECORD_HEADER_BYTES + MAX_RECORD_BYTES;

const HANDSHAKE = 22;
const CLIENT_HELLO = 1;
const SERVER_NAME = 0;
const HOST_NAME = 0;

export type Sni =
  /** More bytes are needed. */
  | { readonly kind: "incomplete" }
  /** A ClientHello, and the name it asks for, if any. */
  | { readonly kind: "hello"; readonly name?: string }
  /** Not a ClientHello: the proxy passes it to the default upstream. */
  | { readonly kind: "invalid" };

const INCOMPLETE: Sni = { kind: "incomplete" };
const INVALID: Sni = { kind: "invalid" };

/** Reads big-endian integers from a slice, failing past its end. */
class Reader {
  offset = 0;
  constructor(readonly bytes: Uint8Array) {}

  get left(): number {
    return this.bytes.length - this.offset;
  }

  uint(size: 1 | 2 | 3): number | undefined {
    if (this.left < size) return undefined;
    let value = 0;
    for (let i = 0; i < size; i++) {
      value = value * 256 + this.bytes[this.offset + i];
    }
    this.offset += size;
    return value;
  }

  take(length: number): Uint8Array | undefined {
    if (length < 0 || this.left < length) return undefined;
    const slice = this.bytes.subarray(this.offset, this.offset + length);
    this.offset += length;
    return slice;
  }

  /** A field with a length prefix of `size` bytes. */
  vector(size: 1 | 2 | 3): Uint8Array | undefined {
    const length = this.uint(size);
    return length === undefined ? undefined : this.take(length);
  }
}

/** Parses what a client sent first. */
export function parseSni(bytes: Uint8Array): Sni {
  if (bytes.length < RECORD_HEADER_BYTES) {
    return bytes.length > 0 && bytes[0] !== HANDSHAKE ? INVALID : INCOMPLETE;
  }
  const header = new Reader(bytes);
  const type = header.uint(1)!;
  header.uint(2); // The record's version, which says nothing.
  const length = header.uint(2)!;
  if (type !== HANDSHAKE || length === 0 || length > MAX_RECORD_BYTES) {
    return INVALID;
  }
  const record = header.take(length);
  if (!record) return INCOMPLETE;
  return parseHello(new Reader(record));
}

function parseHello(record: Reader): Sni {
  if (record.uint(1) !== CLIENT_HELLO) return INVALID;
  const length = record.uint(3);
  // A ClientHello spread over several records is not read: TLS clients
  // send one record, and the proxy then uses the default upstream.
  const hello = length === undefined ? undefined : record.take(length);
  if (!hello) return INVALID;
  const reader = new Reader(hello);
  const skipped = reader.take(2) && // client version
    reader.take(32) && // random
    reader.vector(1) && // session ID
    reader.vector(2) && // cipher suites
    reader.vector(1); // compression methods
  if (!skipped) return INVALID;
  if (reader.left === 0) return { kind: "hello" };
  const extensions = reader.vector(2);
  if (!extensions) return INVALID;
  const list = new Reader(extensions);
  while (list.left > 0) {
    const type = list.uint(2);
    const data = list.vector(2);
    if (type === undefined || !data) return INVALID;
    if (type === SERVER_NAME) return serverName(new Reader(data));
  }
  return { kind: "hello" };
}

function serverName(extension: Reader): Sni {
  const names = extension.vector(2);
  if (!names) return INVALID;
  const list = new Reader(names);
  while (list.left > 0) {
    const type = list.uint(1);
    const name = list.vector(2);
    if (type === undefined || !name) return INVALID;
    if (type !== HOST_NAME) continue;
    const text = new TextDecoder("ascii", { fatal: false }).decode(name);
    return /^[a-z0-9.-]{1,253}$/i.test(text)
      ? { kind: "hello", name: text.toLowerCase() }
      : INVALID;
  }
  return { kind: "hello" };
}

/** Where the proxy sends a name: a node's HTTPS port, or the dashboard. */
export function upstreamPort(
  name: string | undefined,
  ports: {
    readonly dashboard: number;
    readonly node: (index: number) => number;
  },
  maxNodes: number,
): number {
  const match = name === undefined
    ? null
    : /^d([1-9]\d{0,3})\.localhost$/.exec(name);
  if (!match) return ports.dashboard;
  const index = Number(match[1]) - 1;
  return index < maxNodes ? ports.node(index) : ports.dashboard;
}
