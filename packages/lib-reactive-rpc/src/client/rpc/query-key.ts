export function getQueryKey(
  path: string[],
  type: "query" | "mutation" | "subscription",
  input: unknown,
) {
  return ["rpc", path, { input, type }]
}
