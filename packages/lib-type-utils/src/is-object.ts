const isObject = (o: unknown): o is Record<string, unknown> =>
  typeof o === "object" && o !== null

export default isObject
