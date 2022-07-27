import { createSignal } from "solid-js"

import type { GlobalFirehoseMessage } from "../../backend/topics/global-firehose"
import { client } from "../utils/trpc"

const [globalFirehose, setGlobalFirehose] = createSignal<
  readonly GlobalFirehoseMessage[]
>([])

client.subscription("ui.globalFirehose", undefined, {
  onNext(globalFirehoseMessage) {
    setGlobalFirehose((globalFirehose) => {
      if (globalFirehoseMessage.type !== "data") return globalFirehose
      return [...globalFirehose, globalFirehoseMessage.data] as const
    })
  },
})

export { globalFirehose, setGlobalFirehose }
