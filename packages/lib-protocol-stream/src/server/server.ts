import type { Connection } from "../connection/connection"
import { generateCredentials } from "./generate-credentials"
import type { ServerState } from "./state"

export class Server {
  constructor(private readonly state: ServerState) {}

  generateCredentials() {
    return generateCredentials(this.state)
  }

  on(eventType: "connection", handler: (connection: Connection) => void) {
    this.state.topics[eventType].on(undefined, handler)
  }
}
