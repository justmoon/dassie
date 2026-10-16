import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RpcClient } from "./mod.ts";
import { ipcConnector } from "./ipc.ts";
import {
  ClientMessageSchema,
  NodeState,
  ServerMessageSchema,
} from "./proto.ts";

/** Reads one length-prefixed frame. */
function readFrame(socket: Socket): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    let buffered = new Uint8Array(0);
    const onData = (chunk: Uint8Array) => {
      const joined = new Uint8Array(buffered.length + chunk.length);
      joined.set(buffered);
      joined.set(chunk, buffered.length);
      buffered = joined;
      if (buffered.length < 4) return;
      const length = new DataView(buffered.buffer).getUint32(0);
      if (buffered.length < 4 + length) return;
      socket.off("data", onData);
      resolve(buffered.slice(4, 4 + length));
    };
    socket.on("data", onData);
    socket.once("error", reject);
  });
}

/** Writes `bytes`, once the socket has taken them. */
function write(socket: Socket, bytes: Uint8Array): Promise<void> {
  return new Promise((resolve, reject) => {
    socket.write(bytes, (error) => error ? reject(error) : resolve());
  });
}

test("calls go over the IPC socket in length-prefixed frames", async () => {
  const dir = await mkdtemp(join(tmpdir(), "dassie-ipc-"));
  const path = `${dir}/ipc.sock`;
  const listener = createServer();
  await new Promise<void>((resolve, reject) => {
    listener.once("error", reject);
    listener.listen(path, resolve);
  });
  const server = new Promise<Socket>((resolve) => {
    listener.once("connection", resolve);
  }).then(async (connection) => {
    const request = fromBinary(
      ClientMessageSchema,
      await readFrame(connection),
    );
    const reply = toBinary(
      ServerMessageSchema,
      create(ServerMessageSchema, {
        callId: request.callId,
        kind: {
          case: "response",
          value: {
            method: {
              case: "getBasicState",
              value: { state: NodeState.AUTHENTICATED, nodeId: "dtest" },
            },
          },
        },
      }),
    );
    // Split the frame, as a stream may.
    const frame = new Uint8Array(4 + reply.length);
    new DataView(frame.buffer).setUint32(0, reply.length);
    frame.set(reply, 4);
    await write(connection, frame.subarray(0, 3));
    await write(connection, frame.subarray(3));
    connection.end();
  });
  const client = new RpcClient({ url: "ipc", connect: ipcConnector(path) });
  const state = await client.call("getBasicState", {});
  assertEquals(state.nodeId, "dtest");
  assertEquals(state.state, NodeState.AUTHENTICATED);
  client.close();
  await server;
  await new Promise((resolve) => listener.close(resolve));
  await rm(dir, { recursive: true, force: true });
});
