import { Effect, createReactor } from "../reactor"
import { createTopic } from "../topic"

const rootEffect: Effect = (sig) => {
  sig.use(logger)
  sig.use(greeter, { toGreet: "world" })
  sig.use(greeter, { toGreet: "moms" })
}

interface ChildProperties {
  toGreet: string
}

const outputTopic = () => createTopic<string>()

const greeter: Effect<ChildProperties> = (sig, { toGreet }) => {
  sig.emit(outputTopic, `Hello ${toGreet}!`)
}

const logger: Effect = (sig) => {
  sig.on(outputTopic, console.log)
}

createReactor(rootEffect)
