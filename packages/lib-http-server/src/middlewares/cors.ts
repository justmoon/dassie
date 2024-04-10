import { Middleware } from "../router"

export const cors: Middleware<object, object> = ({ response: { headers } }) => {
  headers.set("Access-Control-Allow-Origin", "*")
}
