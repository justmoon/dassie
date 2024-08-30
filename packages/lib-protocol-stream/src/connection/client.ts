import { UINT64_MAX } from "@dassie/lib-oer"

import { getPskEnvironment } from "../crypto/functions"
import { Connection } from "./connection"
import type { ConnectionContext } from "./context"
import type { ConnectionState } from "./state"

interface ClientOptions {
  context: ConnectionContext
  destination: string
  secret: Uint8Array
}

const DEFAULT_MAXIMUM_PACKET_AMOUNT = UINT64_MAX

export const createInitialClientState = ({
  context,
  destination,
  secret,
}: ClientOptions): ConnectionState => {
  return {
    context,
    remoteAddress: destination,
    pskEnvironment: getPskEnvironment(context.crypto, secret),
    exchangeRate: undefined,
    nextSequence: 0,
    // Stream IDs are odd for clients and even for servers
    nextStreamId: 1,
    streams: new Map(),
    maximumPacketAmount: DEFAULT_MAXIMUM_PACKET_AMOUNT,
  }
}

export const createClient = ({
  context,
  destination,
  secret,
}: ClientOptions): Connection => {
  const state = createInitialClientState({ context, destination, secret })
  return new Connection(state)
}
