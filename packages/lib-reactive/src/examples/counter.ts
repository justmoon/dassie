import { createActor } from "../actor"
import { Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

const CountSignal = () => createSignal(0)

const ClockActor = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.use(CountSignal).update((state) => state + 1)
    }, 75)
  })

const LoggerActor = () =>
  createActor((sig) => {
    sig.on(CountSignal, (state) => {
      console.info(`the counter is: ${state}`)
    })
  })

const RootActor = (reactor: Reactor) =>
  createActor((sig) => {
    sig.run(ClockActor)
    sig.run(LoggerActor)
    sig.timeout(() => void reactor.lifecycle.dispose(), 400)
  })

createReactor(RootActor)
