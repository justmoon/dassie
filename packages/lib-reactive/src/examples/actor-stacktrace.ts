import {
  Reactor,
  createActor,
  createMapped,
  createReactor,
  createSignal,
} from ".."

const InnerActor = () =>
  createActor(() => {
    throw new Error("Intentional error")
  })

const SomeSet = () => createSignal(new Set([undefined]))

const SetActors = (reactor: Reactor) =>
  createMapped(reactor.lifecycle, reactor.use(SomeSet), () =>
    createActor((sig) => {
      sig.run(InnerActor)
    }),
  )

const RootActor = () =>
  createActor((sig) => {
    sig.runMap(SetActors)
  })

createReactor(RootActor)
