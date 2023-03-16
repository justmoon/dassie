import type { Reactor } from "@dassie/lib-reactive"

import start from "../src"

interface GlobalWithReactor {
  reactor?: Reactor | undefined
  latest?: symbol
}

const context = global as GlobalWithReactor

const ourSymbol = Symbol()
context.latest = ourSymbol

if (context.reactor) {
  await context.reactor.dispose()
}

if (context.latest === ourSymbol) {
  context.reactor = start()
}
