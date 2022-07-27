import { createServer } from "node:http"

import { createReactor } from "../reactor"
import { createStore } from "../store"
import { createValue } from "../value"

const config = () => createStore({ port: 3000 })

const httpServerFactory = () =>
  createValue((sig) => {
    const port = sig.get(config, ({ port }) => port)

    const server = createServer()
    server.listen(port)

    sig.onCleanup(() => {
      server.close()
    })

    console.log(`http-server listening on port ${port}`)

    return server
  })

createReactor((sig) => {
  sig.read(httpServerFactory)

  sig.timeout(
    () => sig.emit(config, (config) => ({ ...config, port: 3100 })),
    1000
  )
})
