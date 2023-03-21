import { createServer } from "node:http"

import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createService } from "../service"
import { createSignal } from "../signal"

const config = () => createSignal({ port: 3000 })

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

const rootActor = () =>
  createActor((sig) => {
    sig.run(sig.use(httpService).effect)

    sig.timeout(
      () => sig.use(config).update((config) => ({ ...config, port: 3100 })),
      1000
    )

    sig.timeout(() => void sig.reactor.dispose(), 2000)
  })

createReactor(rootActor)
