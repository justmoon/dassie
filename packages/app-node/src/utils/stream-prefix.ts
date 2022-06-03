const prefix = (prefix: string) =>
  async function* (chunks: AsyncIterable<string>): AsyncIterable<string> {
    for await (const chunk of chunks) {
      yield `${prefix}${chunk}`
    }
  }

export default prefix
