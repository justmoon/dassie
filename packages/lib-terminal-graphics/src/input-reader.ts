import { on } from "node:events"
import { stdin } from "node:process"
import { Key, emitKeypressEvents } from "node:readline"
import { ReadStream } from "node:tty"

interface InputReaderOptions {
  inputStream: ReadStream
}

export const createInputReader = ({
  inputStream = stdin,
}: InputReaderOptions) => {
  const abortController = new AbortController()

  return {
    async *keys() {
      emitKeypressEvents(inputStream)
      if (inputStream.isTTY) {
        inputStream.setRawMode(true)
      }
      inputStream.resume()

      try {
        for await (const key of on(inputStream, "keypress", {
          signal: abortController.signal,
        })) {
          yield (key as [sequence: string, key: Key])[1]
        }
      } catch (error) {
        if (error instanceof Error && error.name === "AbortError") {
          return
        }

        throw error
      } finally {
        if (inputStream.isTTY) {
          inputStream.setRawMode(false)
        }
        inputStream.pause()
      }
    },
    abort() {
      abortController.abort()
    },
  }
}
