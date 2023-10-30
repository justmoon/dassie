async function* splitByLine(
  chunks: AsyncIterable<string>,
): AsyncIterable<string> {
  let buffer = ""
  for await (const chunk of chunks) {
    buffer += chunk
    let index
    while ((index = buffer.indexOf("\n")) > -1) {
      const line = buffer.slice(0, index)
      yield line
      buffer = buffer.slice(index + 1)
    }
  }

  if (buffer.length > 0) {
    yield buffer
  }
}

export default splitByLine
