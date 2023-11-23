import { Middleware } from "./router"

export const cors: Middleware<object, object> = (_request, response) => {
  response.setHeader("Access-Control-Allow-Origin", "*")
}
