import { createServer } from "node:http"

import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const config = () => createSignal({ port: 3000 })

const httpService = () =>
  createActor((sig) => {
    const port = sig.get(config, ({ port }) => port)

    const server = createServer()
    server.listen(port)

    sig.onCleanup(() => {
      server.close()
    })

    console.info(`http-server listening on port ${port}`)

    return server
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(httpService)

    sig.timeout(() => {
      sig.use(config).update((config) => ({ ...config, port: 3100 }))
    }, 1000)

    sig.timeout(() => void sig.reactor.dispose(), 2000)
  })

createReactor(rootActor)
