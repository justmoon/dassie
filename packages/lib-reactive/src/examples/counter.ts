import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const counterSignal = () => createSignal(0)

const clock = (sig: EffectContext) => {
  sig.interval(() => {
    sig.use(counterSignal).update((state) => state + 1)
  }, 75)
}

const logger = (sig: EffectContext) => {
  sig.on(counterSignal, (state) => {
    console.log(`the counter is: ${state}`)
  })
}

createReactor((sig) => {
  sig.run(clock)
  sig.run(logger)
  sig.timeout(() => void sig.reactor.dispose(), 400)
})
