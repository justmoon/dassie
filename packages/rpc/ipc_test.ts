import { assertEquals } from "@std/assert";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { RpcClient } from "./mod.ts";
import { ipcConnector } from "./ipc.ts";
import {
  ClientMessageSchema,
  NodeState,
  ServerMessageSchema,
} from "./proto.ts";

/** Reads one length-prefixed frame. */
async function readFrame(connection: Deno.Conn): Promise<Uint8Array> {
  const header = new Uint8Array(4);
  let read = 0;
  while (read < 4) read += (await connection.read(header.subarray(read)))!;
  const length = new DataView(header.buffer).getUint32(0);
  const body = new Uint8Array(length);
  read = 0;
  while (read < length) read += (await connection.read(body.subarray(read)))!;
  return body;
}

Deno.test("calls go over the IPC socket in length-prefixed frames", async () => {
  const dir = await Deno.makeTempDir();
  const path = `${dir}/ipc.sock`;
  const listener = Deno.listen({ transport: "unix", path });
  const server = (async () => {
    const connection = await listener.accept();
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
    await connection.write(frame.subarray(0, 3));
    await connection.write(frame.subarray(3));
    connection.close();
  })();
  const client = new RpcClient({ url: "ipc", connect: ipcConnector(path) });
  const state = await client.call("getBasicState", {});
  assertEquals(state.nodeId, "dtest");
  assertEquals(state.state, NodeState.AUTHENTICATED);
  client.close();
  await server;
  listener.close();
  await Deno.remove(dir, { recursive: true });
});
