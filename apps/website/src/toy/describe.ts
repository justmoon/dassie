/**
 * Words for what the STREAM toy shows: ILP error codes, STREAM error codes
 * and frames, decrypted with the connection's shared secret.
 *
 * @module
 */

import type { decodeAmountTooLarge, decodeStreamPacket } from "@dassie/wasm";

/** The ILP error codes of RFC 27, by code. */
const ILP_ERRORS: Record<string, string> = {
  F00: "Bad Request",
  F01: "Invalid Packet",
  F02: "Unreachable",
  F03: "Invalid Amount",
  F04: "Insufficient Destination Amount",
  F05: "Wrong Condition",
  F06: "Unexpected Payment",
  F07: "Cannot Receive",
  F08: "Amount Too Large",
  F99: "Application Error",
  T00: "Internal Error",
  T01: "Peer Unreachable",
  T02: "Peer Busy",
  T03: "Connector Busy",
  T04: "Insufficient Liquidity",
  T05: "Rate Limited",
  T99: "Application Error",
  R00: "Transfer Timed Out",
  R01: "Insufficient Source Amount",
  R02: "Insufficient Timeout",
  R99: "Application Error",
};

/** An ILP error code's name, such as "Amount Too Large" for F08. */
export function ilpErrorName(code: string): string {
  return ILP_ERRORS[code] ?? "Unknown Error";
}

/** The STREAM error codes of RFC 29 §5.4, by code. */
const STREAM_ERRORS = [
  undefined,
  "NoError",
  "InternalError",
  "EndpointBusy",
  "FlowControlError",
  "StreamIdError",
  "StreamStateError",
  "FrameFormatError",
  "ProtocolViolation",
  "ApplicationError",
];

/** A STREAM error code's name, such as "NoError" for 1. */
export function streamErrorName(code: number): string {
  return STREAM_ERRORS[code] ??
    `Unknown (0x${code.toString(16).padStart(2, "0")})`;
}

/** A STREAM frame, read out of its WASM view. */
export interface Frame {
  kind: string;
  streamId: bigint;
  code: number;
  text: string;
  first: bigint;
  second: bigint;
  length: number;
}

/** A decrypted STREAM packet, read out of its WASM view. */
export interface StreamPacket {
  /** The ILP packet type it travels on: 12, 13 or 14. */
  type: number;
  sequence: bigint;
  prepareAmount: bigint;
  frames: Frame[];
}

/** What decoding needs of the STREAM module. */
export interface Decoders {
  decodeStreamPacket: typeof decodeStreamPacket;
  decodeAmountTooLarge: typeof decodeAmountTooLarge;
}

/**
 * Decrypts and parses STREAM data, or says why it cannot: empty data, or
 * data that is not a STREAM packet of this connection.
 */
export function decodeStream(
  decoders: Decoders,
  sharedSecret: Uint8Array,
  data: Uint8Array,
): StreamPacket | string {
  if (data.length === 0) return "No data";
  let view;
  try {
    view = decoders.decodeStreamPacket(sharedSecret, data);
  } catch {
    return "Not a STREAM packet of this connection";
  }
  // The frames are copies, each to be freed, whatever happens.
  const frames = view.frames;
  try {
    return {
      type: view.type,
      sequence: view.sequence,
      prepareAmount: view.prepareAmount,
      frames: frames.map((frame) => ({
        kind: frame.kind,
        streamId: frame.streamId,
        code: frame.code,
        text: frame.text,
        first: frame.first,
        second: frame.second,
        length: frame.length,
      })),
    };
  } finally {
    for (const frame of frames) frame.free();
    view.free();
  }
}

/** The amounts of an F08 reject's data, or why there are none. */
export function decodeTooLarge(
  decoders: Decoders,
  data: Uint8Array,
): { received: bigint; maximum: bigint } | string {
  const amounts = decoders.decodeAmountTooLarge(data);
  if (!amounts) return "No amounts in the data";
  return { received: amounts[0], maximum: amounts[1] };
}

/** A frame's type, as RFC 29 names it: "StreamMoney" and so on. */
export function frameName(frame: Frame): string {
  return frame.kind.charAt(0).toUpperCase() + frame.kind.slice(1);
}

/** What a frame says, in words. */
export function describeFrame(frame: Frame): string {
  const stream = `stream ${frame.streamId}`;
  const reason = (frame: Frame) =>
    `${streamErrorName(frame.code)}${frame.text ? `: ${frame.text}` : ""}`;
  switch (frame.kind) {
    case "connectionClose":
      return reason(frame);
    case "connectionNewAddress":
      return frame.text || "(no address)";
    case "connectionAssetDetails":
      return `${frame.text}, scale ${frame.first}`;
    case "connectionMaxData":
    case "connectionDataBlocked":
      return `offset ${frame.first}`;
    case "connectionMaxStreamId":
    case "connectionStreamIdBlocked":
      return `stream ID ${frame.first}`;
    case "streamClose":
      return `${stream}, ${reason(frame)}`;
    case "streamMoney":
      return `${stream}: ${frame.first} shares`;
    case "streamMaxMoney":
      return `${stream}: receives up to ${frame.first}, has ${frame.second}`;
    case "streamMoneyBlocked":
      return `${stream}: wants to send ${frame.first}, has sent ${frame.second}`;
    case "streamData":
      return `${stream}: ${frame.length} bytes at offset ${frame.first}`;
    case "streamMaxData":
    case "streamDataBlocked":
      return `${stream}: offset ${frame.first}`;
    case "streamReceipt":
      return `${stream}: ${frame.length} bytes`;
    default:
      return "";
  }
}
