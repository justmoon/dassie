import { hasWorkToDo } from "./has-work-to-do"
import { sendOnce } from "./send-once"
import type { ConnectionState } from "./state"

const PACKET_DELAY = 100

export function sendUntilDone(state: ConnectionState) {
  // If there is already a send loop active, we don't need to start another one
  if (state.isSending) return

  state.context.logger.debug?.("starting send loop")
  state.isSending = true
  ;(async function sendLoop() {
    for (;;) {
      await sendOnce(state)

      if (!hasWorkToDo(state)) break

      // TODO: This is just temporary - eventually there should be a send
      // scheduler which decides maximum number of packets in flight etc.
      await new Promise((resolve) => setTimeout(resolve, PACKET_DELAY))
    }
  })().catch((error: unknown) => {
    console.error("error in send loop", { error })
  })
}
