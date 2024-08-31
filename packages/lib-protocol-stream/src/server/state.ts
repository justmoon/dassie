import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import type { Topic } from "@dassie/lib-reactive"

import type { Connection } from "../connection/connection"
import type { ConnectionState } from "../connection/state"
import type { StreamProtocolContext } from "../context/context"

export interface ServerState {
  readonly context: StreamProtocolContext
  readonly activeCredentials: Map<string, Uint8Array>
  readonly activeConnections: Map<string, ConnectionState>
  configuration: IldcpResponse
  topics: {
    connection: Topic<Connection>
  }
}
