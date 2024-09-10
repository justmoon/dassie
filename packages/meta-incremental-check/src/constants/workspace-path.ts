import path from "node:path"

export const WORKSPACE_ROOT_PATH = path.resolve(
  new URL(import.meta.url).pathname,
  "../../../../",
)
