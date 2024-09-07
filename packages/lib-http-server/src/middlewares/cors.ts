import type { Middleware } from "../router"

export const cors: Middleware<{}, object> = ({ response: { headers } }) => {
  headers.set("Access-Control-Allow-Origin", "*")
}
