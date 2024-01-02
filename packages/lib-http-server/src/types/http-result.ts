import { ServerResponse } from "node:http"

export interface HttpResult {
  applyTo: (response: ServerResponse) => void
}
