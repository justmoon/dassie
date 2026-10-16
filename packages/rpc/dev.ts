/**
 * The generated messages of the dev environment's control channel
 * `proto/dassie/dev/v1`, between the dev server and its dashboard, and
 * their framing: one message per WebSocket binary message, of at most
 * {@linkcode MAX_CONTROL_MESSAGE_BYTES}.
 *
 * @module
 */

import { fromBinary, toBinary } from "@bufbuild/protobuf";
import {
  type ClientMessage,
  ClientMessageSchema,
  type ServerMessage,
  ServerMessageSchema,
} from "./src/gen/dassie/dev/v1/control_pb.ts";

export * from "./src/gen/dassie/dev/v1/control_pb.ts";

/** The largest message either side sends or accepts. */
export const MAX_CONTROL_MESSAGE_BYTES = 1024 * 1024;

function bounded<T extends Uint8Array>(bytes: T): T {
  if (bytes.length > MAX_CONTROL_MESSAGE_BYTES) {
    throw new RangeError(`a control message of ${bytes.length} bytes`);
  }
  return bytes;
}

/** A server message as bytes; throws if it is too large. */
export function encodeServerMessage(
  message: ServerMessage,
): Uint8Array<ArrayBuffer> {
  return bounded(toBinary(ServerMessageSchema, message));
}

/** A server message from bytes; throws if they are too many or not one. */
export function decodeServerMessage(bytes: Uint8Array): ServerMessage {
  return fromBinary(ServerMessageSchema, bounded(bytes));
}

/** A client message as bytes; throws if it is too large. */
export function encodeClientMessage(
  message: ClientMessage,
): Uint8Array<ArrayBuffer> {
  return bounded(toBinary(ClientMessageSchema, message));
}

/** A client message from bytes; throws if they are too many or not one. */
export function decodeClientMessage(bytes: Uint8Array): ClientMessage {
  return fromBinary(ClientMessageSchema, bounded(bytes));
}
