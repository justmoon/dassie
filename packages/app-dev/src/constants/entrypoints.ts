export const DEVELOPMENT_SERVER_ENTRYPOINT = new URL(
  "../start.ts",
  import.meta.url,
).pathname

export const NODE_ENTRYPOINT = new URL(
  "../runner/launchers/node.ts",
  import.meta.url,
).pathname
