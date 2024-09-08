import { useEffect, useState } from "react"

import {
  type PskEnvironment,
  type StreamPacket,
  streamPacketSchema,
} from "@dassie/lib-protocol-stream"
import { Failure, isFailure } from "@dassie/lib-type-utils"

export class StreamPacketParseFailure extends Failure {
  readonly name = "StreamPacketParseFailure"

  constructor(readonly reason: string) {
    super()
  }
}

export function useParsedStreamPacket(
  data: Uint8Array | undefined,
  pskEnvironment: PskEnvironment,
) {
  const [parsedStreamPacket, setParsedStreamPacket] = useState<
    StreamPacket | StreamPacketParseFailure | undefined
  >()

  useEffect(() => {
    setParsedStreamPacket(undefined)

    if (!data) return

    if (data.length === 0) {
      setParsedStreamPacket(new StreamPacketParseFailure("No data"))
      return
    }

    pskEnvironment
      .decrypt(data)
      .then((decryptedData) => {
        if (isFailure(decryptedData)) {
          setParsedStreamPacket(
            new StreamPacketParseFailure(
              "Decryption failed (probably not STREAM data)",
            ),
          )
          return
        }

        const parseResult = streamPacketSchema.parse(decryptedData)

        if (isFailure(parseResult)) {
          setParsedStreamPacket(new StreamPacketParseFailure("Parsing failed"))
          return
        }

        setParsedStreamPacket(parseResult.value)
      })
      .catch((error) => {
        setParsedStreamPacket(new StreamPacketParseFailure(error.message))
      })
  }, [data, pskEnvironment, setParsedStreamPacket])

  return parsedStreamPacket
}