import { createReactor } from "@dassie/lib-reactive"

import { main } from "."

const reactor = createReactor()
await main(reactor)
await reactor.dispose()
