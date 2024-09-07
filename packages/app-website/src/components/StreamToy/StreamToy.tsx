import { useState } from "react"

import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import { Input } from "@dassie/app-node/src/frontend/components/ui/input"
import { Label } from "@dassie/app-node/src/frontend/components/ui/label"
import {
  createClient,
  createServer,
  createTestEnvironment,
} from "@dassie/lib-protocol-stream"
import { unwrapFailure } from "@dassie/lib-type-utils"

export default function StreamToy() {
  const [amount, setAmount] = useState(1000n)
  const [maxPacketAmount, setMaxPacketAmount] = useState(1000n)

  async function simulate() {
    try {
      const environment = createTestEnvironment({
        maxPacketAmount,
      })

      const server = unwrapFailure(
        await createServer({
          context: environment.createContext({ name: "server" }),
        }),
      )

      const credentials = server.generateCredentials()

      const client = unwrapFailure(
        await createClient({
          context: environment.createContext({ name: "client" }),
          credentials,
        }),
      )

      const stream = client.createStream()

      stream.send(amount)

      await client.flush()

      console.log("done")
    } catch (error: unknown) {
      console.error("error while sending", { error })
    }
  }

  return (
    <div className="p-6 gap-6 flex flex-col">
      <h1 className="font-bold text-4xl">STREAM Toy</h1>
      <div className="flex flex-col gap-4 border p-4 rounded-md">
        <h2 className="text-2xl">Simulated Interledger Settings</h2>
        <div className="grid w-full max-w-sm items-center gap-1.5">
          <Label htmlFor="max_packet_amount">Max. Packet Amount</Label>
          <Input
            type="number"
            id="max_packet_amount"
            value={String(maxPacketAmount)}
            onChange={(event) => setMaxPacketAmount(BigInt(event.target.value))}
          />
        </div>
      </div>
      <div className="flex flex-col gap-4 border p-4 rounded-md">
        <h2 className="text-2xl">Payment Settings</h2>
        <div className="grid w-full max-w-sm items-center gap-1.5">
          <Label htmlFor="send_amount">Send Amount</Label>
          <Input
            type="number"
            id="send_amount"
            value={String(amount)}
            onChange={(event) => setAmount(BigInt(event.target.value))}
          />
        </div>
      </div>
      <Button onClick={() => simulate()}>Simulate</Button>
    </div>
  )
}
