export const compareSetOfKeys = (
  a: Map<string, unknown>,
  b: Map<string, unknown>
) => {
  if (a.size !== b.size) return false

  for (const key of a.keys()) {
    if (!b.has(key)) return false
  }

  return true
}
