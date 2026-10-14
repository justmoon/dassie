// The part of `ws` 6 the end-to-end tests use. Its own types pull in
// @types/node 10, which clashes with Deno's.

/** A WebSocket client of the `ws` package. */
declare class WebSocket {
  constructor(url: string, options?: { ca?: string });
}

export default WebSocket;
