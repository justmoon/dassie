import { createReactor } from "../create-reactor"
import { createStore } from "../create-store"
import type { EffectContext } from "../use-effect"

const counterStore = createStore("counter", 0)

const clock = (sig: EffectContext) => {
  sig.interval(() => {
    sig.reactor.emit(counterStore, (state) => state + 1)
  }, 75)
}

const logger = (sig: EffectContext) => {
  sig.on(counterStore, (state) => {
    console.log(`the counter is: ${state}`)
  })
}

createReactor((sig: EffectContext) => {
  sig.use(clock)
  sig.use(logger)
  sig.timeout(() => void sig.reactor.dispose(), 400)
})
