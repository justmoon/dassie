import assert from "node:assert"

export const getLedgerIdFromPath = (path: string) => {
  const firstSlash = path.indexOf("/")

  assert(firstSlash !== -1, "account paths must contain at least one slash")

  return path.slice(0, firstSlash)
}
