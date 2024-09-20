import { header } from "../components/header"
import { text } from "../components/text"
import { createFlow } from "../flow"

const flow = createFlow()

flow.show(header({ title: "Example: Text Input" }))

await flow.interact(
  text({
    title: "Type something...",
  }),
)
