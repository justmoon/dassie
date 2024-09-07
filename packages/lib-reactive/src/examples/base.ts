import { type ActorContext, createActor, createReactor } from ".."

interface Io {
  read(): string
}

const ActorNeedingIo = () =>
  createActor((sig: ActorContext<{ io: Io }>) => {
    console.info(sig.base.io.read())
  })

const RootActor = () =>
  createActor((sig) => {
    const io: Io = {
      read() {
        return "foo"
      },
    }
    sig.withBase({ io }).run(ActorNeedingIo)
  })

createReactor(RootActor)
