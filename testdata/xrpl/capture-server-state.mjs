// Captures crates/dassie-xrpl/tests/data/server-state.json
// (spec/designs/xrpl-validation.md#13-what-pins-it), read-only: the whole
// reply to `server_state` at the default API version, 1, over WebSocket,
// of mainnet's and the testnet's public servers and of each network's
// proof server, the network's default.
//
// Usage, from the repository root:
//
//   node testdata/xrpl/capture-server-state.mjs > crates/dassie-xrpl/tests/data/server-state.json

import { assert, open, print } from "./capture-lib.mjs";

const SERVERS = [
  "wss://s1.ripple.com",
  "wss://s2.ripple.com",
  "wss://xrplcluster.com",
  "wss://s.altnet.rippletest.net:51233",
  "wss://mainnet.xrpl.dassieland.net",
  "wss://testnet.xrpl.dassieland.net",
];

const captured = new Date().toISOString();
const servers = {};
for (const url of SERVERS) {
  const connection = await open(url);
  // At the default API version, 1, which the reply then does not echo.
  const reply = await connection.request({ command: "server_state" });
  connection.close();
  assert(reply.status === "success" && reply.result?.state !== undefined, `${url} answered server_state`);
  servers[url] = reply;
}

print({
  source: `server_state from public servers and from the project's proof servers, over WebSocket (rippled API v1). Captured ${captured} by testdata/xrpl/capture-server-state.mjs.`,
  servers,
});
