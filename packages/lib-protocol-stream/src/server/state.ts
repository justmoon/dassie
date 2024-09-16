import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import type { DisposableScope } from "@dassie/lib-reactive"

import type { Connection } from "../connection/connection"
import type { ConnectionState } from "../connection/state"
import type { StreamProtocolContext } from "../context/context"
import type { InferTopics } from "../types/infer-topics"

export type ServerEvents = {
  connection: Connection
}

export interface ServerState {
  readonly context: StreamProtocolContext
  readonly scope: DisposableScope
  readonly activeCredentials: Map<string, Uint8Array>
  readonly activeConnections: Map<string, ConnectionState>
  configuration: IldcpResponse
  topics: InferTopics<ServerEvents>
}
