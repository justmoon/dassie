// The part of `ws` 6 the end-to-end tests use, which they import as `#ws`
// (see `imports` in package.json). `ws` 6 has no types, and a
// `declare module "ws"` would stand in for the dev environment's `ws` 8 too.

/** A WebSocket client of the `ws` package. */
declare class WebSocket {
  constructor(url: string, options?: { ca?: string });
}

export default WebSocket;
