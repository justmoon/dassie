export function parseParameters(
  urlPath: string,
  pattern: string[],
  parameterMap: string[],
): Record<string, string> {
  const normalizedPath = urlPath
    .replace(/^\//, "")
    .replace(/\/$/, "")
    .split("/")

  const parameters: Record<string, string> = {}
  let parameterIndex = 0
  for (const [index, segment] of pattern.entries()) {
    if (segment === "?") {
      const value = normalizedPath[index]

      if (value === undefined) {
        throw new Error("URL path does not match pattern")
      }

      const parameterName = parameterMap[parameterIndex++]

      if (!parameterName) {
        throw new Error("URL path does not match pattern")
      }

      parameters[parameterName] = value
    } else if (segment === "*") {
      const value = normalizedPath.slice(index).join("/")

      const parameterName = parameterMap[parameterIndex++]

      if (!parameterName) {
        throw new Error("URL path does not match pattern")
      }

      parameters[parameterName] = value
      break
    }
  }

  return parameters
}
