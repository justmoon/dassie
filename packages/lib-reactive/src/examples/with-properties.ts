import { Effect, createReactor } from "../create-reactor"
import { createTopic } from "../create-topic"

const rootEffect: Effect = (sig) => {
  sig.use(logger)
  sig.use(greeter, { toGreet: "world" })
  sig.use(greeter, { toGreet: "moms" })
}

interface ChildProperties {
  toGreet: string
}

const outputTopic = createTopic<string>("output")

const greeter: Effect<ChildProperties> = (sig, { toGreet }) => {
  sig.reactor.emit(outputTopic, `Hello ${toGreet}!`)
}

const logger: Effect = (sig) => {
  sig.on(outputTopic, console.log)
}

createReactor(rootEffect)
