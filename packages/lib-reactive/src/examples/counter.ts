import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createStore } from "../store"

const counterStore = () => createStore(0)

const clock = (sig: EffectContext) => {
  sig.interval(() => {
    sig.emit(counterStore, (state) => state + 1)
  }, 75)
}

const logger = (sig: EffectContext) => {
  sig.on(counterStore, (state) => {
    console.log(`the counter is: ${state}`)
  })
}

createReactor((sig) => {
  sig.run(clock)
  sig.run(logger)
  sig.timeout(() => void sig.reactor.dispose(), 400)
})
