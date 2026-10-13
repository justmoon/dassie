import { assertEquals, assertRejects } from "@std/assert";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { FakeTime } from "@std/testing/time";
import {
  ErrorCode,
  RpcClient,
  RpcError,
  SESSION_ENDED,
  type SocketLike,
} from "./mod.ts";
import {
  type ClientMessage,
  ClientMessageSchema,
  NodeState,
  type ServerMessage,
  ServerMessageSchema,
} from "./proto.ts";

/** A socket the test drives by hand. */
class FakeSocket implements SocketLike {
  binaryType: BinaryType = "blob";
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  sent: ClientMessage[] = [];

  send(data: Uint8Array): void {
    this.sent.push(fromBinary(ClientMessageSchema, data));
  }

  close(code = 1000): void {
    this.lose(code);
  }

  open(): void {
    this.readyState = 1;
    this.onopen?.(new Event("open"));
  }

  deliver(message: ServerMessage): void {
    const bytes = toBinary(ServerMessageSchema, message);
    this.onmessage?.(
      new MessageEvent("message", { data: bytes.slice().buffer }),
    );
  }

  lose(code: number): void {
    this.readyState = 3;
    this.onclose?.(new CloseEvent("close", { code }));
  }
}

function setUp(options: { onSessionEnded?: () => void } = {}) {
  const sockets: FakeSocket[] = [];
  const client = new RpcClient({
    url: "wss://node.example/rpc",
    callTimeoutMs: 1_000,
    connect: (_url, protocol) => {
      assertEquals(protocol, "dassie.ui.v1");
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
    ...options,
  });
  return { client, sockets };
}

Deno.test("calls wait for the socket, then resolve with their response", async () => {
  const { client, sockets } = setUp();
  const call = client.call("getBasicState", {});
  const [socket] = sockets;
  assertEquals(
    socket.sent.length,
    0,
    "nothing goes out before the socket opens",
  );
  socket.open();
  const [request] = socket.sent;
  assertEquals(request.kind.case, "request");
  socket.deliver(create(ServerMessageSchema, {
    callId: request.callId,
    kind: {
      case: "response",
      value: {
        method: {
          case: "getBasicState",
          value: { state: NodeState.AUTHENTICATED, nodeId: "dX" },
        },
      },
    },
  }));
  const state = await call;
  assertEquals([state.state, state.nodeId], [NodeState.AUTHENTICATED, "dX"]);
  client.close();
});

Deno.test("errors carry the node's code", async () => {
  const { client, sockets } = setUp();
  const call = client.call("getConfig", {});
  sockets[0].open();
  sockets[0].deliver(create(ServerMessageSchema, {
    callId: sockets[0].sent[0].callId,
    kind: {
      case: "error",
      value: { code: ErrorCode.UNAUTHENTICATED, message: "log in first" },
    },
  }));
  const error = await assertRejects(() => call, RpcError);
  assertEquals(error.code, ErrorCode.UNAUTHENTICATED);
  client.close();
});

// RT-27: the original's calls hung forever when a response was lost.
Deno.test("a call that gets no answer times out and is cancelled", async () => {
  using time = new FakeTime();
  const { client, sockets } = setUp();
  const call = client.call("getStatus", {});
  sockets[0].open();
  const rejected = assertRejects(
    () => call,
    RpcError,
    "did not answer in time",
  );
  time.tick(1_000);
  await rejected;
  const [request, cancel] = sockets[0].sent;
  assertEquals([cancel.kind.case, cancel.callId], ["cancel", request.callId]);
  client.close();
});

Deno.test("losing the connection fails calls in flight, and the session end is reported", async () => {
  let ended = 0;
  const { client, sockets } = setUp({ onSessionEnded: () => ended++ });
  const call = client.call("getStatus", {});
  sockets[0].open();
  sockets[0].lose(SESSION_ENDED);
  await assertRejects(() => call, RpcError, "lost");
  assertEquals(ended, 1);
  client.close();
});

Deno.test("streams deliver items until their end", async () => {
  const { client, sockets } = setUp();
  const steps: number[] = [];
  const stream = client.stream("obtainCertificate", {
    agreeToTermsOfService: true,
  }, (item) => {
    steps.push(item.step);
  });
  sockets[0].open();
  const callId = sockets[0].sent[0].callId;
  for (const step of [2, 4]) {
    sockets[0].deliver(create(ServerMessageSchema, {
      callId,
      kind: {
        case: "item",
        value: { method: { case: "obtainCertificate", value: { step } } },
      },
    }));
  }
  sockets[0].deliver(
    create(ServerMessageSchema, { callId, kind: { case: "end", value: {} } }),
  );
  await stream;
  assertEquals(steps, [2, 4]);
  client.close();
});
