import {
  type Reactor,
  createActor,
  createMapped,
  createReactor,
  createSignal,
} from ".."

const InnerActor = () =>
  createActor(() => {
    console.info(new Error("InnerActor behavior").stack)

    return {
      handle: () => {
        console.info(new Error("InnerActor handler").stack)
      },
    }
  })

const SomeSet = () => createSignal(new Set([undefined]))

const SetActors = (reactor: Reactor) =>
  createMapped(reactor, SomeSet, () =>
    createActor((sig) => {
      sig.run(InnerActor)

      const callHandle = async () => {
        await sig.reactor.use(InnerActor).api.handle.ask()
      }

      void callHandle()
    }),
  )

const RootActor = () =>
  createActor((sig) => {
    sig.runMap(SetActors)
  })

createReactor(RootActor)
