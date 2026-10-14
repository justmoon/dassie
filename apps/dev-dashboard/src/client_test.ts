import { assertEquals } from "@std/assert";
import { FakeTime } from "@std/testing/time";
import { create } from "@bufbuild/protobuf";
import {
  decodeClientMessage,
  encodeServerMessage,
  ServerMessageSchema,
} from "@dassie/rpc/dev";
import { type ConnectionStatus, DevClient, type SocketLike } from "./client.ts";

class FakeSocket implements SocketLike {
  binaryType: BinaryType = "blob";
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  sent: Uint8Array[] = [];

  send(data: Uint8Array): void {
    this.sent.push(data);
  }

  close(): void {
    this.readyState = 3;
    this.onclose?.(new CloseEvent("close"));
  }

  open(): void {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }
}

Deno.test("the client reconnects with a growing backoff, and starts over once open", () => {
  using time = new FakeTime();
  const sockets: FakeSocket[] = [];
  const statuses: ConnectionStatus[] = [];
  const client = new DevClient({
    url: "wss://localhost/control",
    connect: () => {
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
    onMessage: () => {},
    onStatus: (status) => statuses.push(status),
    random: () => 1,
  });
  client.start();
  assertEquals(sockets.length, 1);
  sockets[0].close();
  time.tick(499);
  assertEquals(sockets.length, 1);
  time.tick(1);
  assertEquals(sockets.length, 2);
  sockets[1].close();
  time.tick(999);
  assertEquals(sockets.length, 2);
  time.tick(1);
  assertEquals(sockets.length, 3);
  sockets[2].open();
  sockets[2].close();
  time.tick(500);
  assertEquals(sockets.length, 4);
  assertEquals(statuses.slice(0, 3), ["connecting", "closed", "connecting"]);
  client.close();
  time.tick(60_000);
  assertEquals(sockets.length, 4);
});

Deno.test("messages arrive decoded, and commands go out only when open", () => {
  const socket = new FakeSocket();
  const received: string[] = [];
  const client = new DevClient({
    url: "wss://localhost/control",
    connect: () => socket,
    onMessage: (message) => received.push(message.message.case ?? ""),
    onStatus: () => {},
  });
  client.start();
  assertEquals(client.send({ case: "addNode", value: {} }), false);
  socket.open();
  assertEquals(client.send({ case: "addNode", value: {} }), true);
  assertEquals(decodeClientMessage(socket.sent[0]).message.case, "addNode");
  const bytes = encodeServerMessage(create(ServerMessageSchema, {
    message: { case: "traffic", value: { hops: [] } },
  }));
  socket.onmessage?.(
    new MessageEvent("message", { data: bytes.slice().buffer }),
  );
  assertEquals(received, ["traffic"]);
  client.close();
});
