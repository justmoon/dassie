import { createServer } from "node:http"

import { createReactor } from "../reactor"
import { createService } from "../service"
import { createStore } from "../store"

const config = () => createStore({ port: 3000 })

const httpService = () =>
  createService((sig) => {
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
  sig.read(httpService)

  sig.timeout(
    () => sig.emit(config, (config) => ({ ...config, port: 3100 })),
    1000
  )

  sig.timeout(() => void sig.reactor.dispose(), 2000)
})
