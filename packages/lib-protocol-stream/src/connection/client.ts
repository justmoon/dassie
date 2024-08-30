import { getPskEnvironment } from "../crypto/functions"
import { Connection } from "./connection"
import type { ConnectionContext } from "./context"
import type { ConnectionState } from "./state"

interface ClientOptions {
  context: ConnectionContext
  destination: string
  secret: Uint8Array
}

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
