import { createServer } from "node:http"

import { createActor } from "../actor"
import { type Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

const ConfigSignal = () => createSignal({ port: 3000 })

const HttpServiceActor = () =>
  createActor((sig) => {
    const port = sig.readAndTrack(ConfigSignal, ({ port }) => port)

    const server = createServer()
    server.listen(port)

    sig.onCleanup(() => {
      server.close()
    })

    console.info(`http-server listening on port ${port}`)

    return server
  })

const RootActor = (reactor: Reactor) =>
  createActor((sig) => {
    sig.run(HttpServiceActor)

    sig.timeout(() => {
      sig.reactor
        .use(ConfigSignal)
        .update((config) => ({ ...config, port: 3100 }))
    }, 1000)

    sig.timeout(() => void reactor.dispose(), 2000)
  })

createReactor(RootActor)
