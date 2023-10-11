import { createReactor } from "@dassie/lib-reactive"

import { main } from "./command-line"

const reactor = createReactor()
await main(reactor)
await reactor.lifecycle.dispose()
