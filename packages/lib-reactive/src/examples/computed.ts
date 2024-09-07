import { createActor } from "../actor"
import { createComputed } from "../computed"
import { type Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

const CountSignal = () => createSignal(0)

const DoubledSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const value = sig.readAndTrack(CountSignal)
    return value * 2
  })

const QuadrupledSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const value = sig.readAndTrack(DoubledSignal)
    return value * 2
  })

const LogActor = () =>
  createActor((sig) => {
    console.info(sig.readAndTrack(QuadrupledSignal))
  })

const IncrementActor = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.reactor.use(CountSignal).update((x) => x + 1)
    }, 1000)
  })

const RootActor = () =>
  createActor((sig) => {
    sig.run(LogActor)
    sig.run(IncrementActor)
  })

createReactor(RootActor)
