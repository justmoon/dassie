import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const counterSignal = () => createSignal(0)

const clock = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.use(counterSignal).update((state) => state + 1)
    }, 75)
  })

const logger = () =>
  createActor((sig) => {
    sig.on(counterSignal, (state) => {
      console.info(`the counter is: ${state}`)
    })
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(clock)
    sig.run(logger)
    sig.timeout(() => void sig.reactor.dispose(), 400)
  })

createReactor(rootActor)
