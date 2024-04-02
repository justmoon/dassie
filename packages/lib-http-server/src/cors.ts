import { Middleware } from "./router"

export const cors: Middleware<object, object> = ({ headers }) => {
  headers.set("Access-Control-Allow-Origin", "*")
}
