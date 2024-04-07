import type { HttpResponse } from "../types/http-response"

/**
 * WebSocket requests are upgrade requests, so this is really just a dummy value
 * that isn't actually used in practice.
 */
export class WebSocketResponse implements HttpResponse {
  asResponse(): Response {
    throw new Error(
      "WebSocketResponse should not be used in a non-websocket request",
    )
  }
}
