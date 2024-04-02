export function makeHeadersContext(headers: Record<string, string>) {
  return {
    request: new Request("http://0.0.0.0/", {
      headers: new Headers({
        ...headers,
      }),
    }),
  }
}
