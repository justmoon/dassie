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

export const compareKeysToArray = (a: Map<string, unknown>, b: string[]) => {
  if (a.size !== b.length) return false

  for (const key of b) {
    if (!a.has(key)) return false
  }

  return true
}

export const compareArrays = (a: string[], b: string[]) => {
  if (a.length !== b.length) return false

  for (const key of b) {
    if (!a.includes(key)) return false
  }

  return true
}
