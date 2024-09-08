import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import { Input } from "@dassie/app-node/src/frontend/components/ui/input"
import { Label } from "@dassie/app-node/src/frontend/components/ui/label"

export interface StreamConfiguration {
  amount: bigint
  maxPacketAmount: bigint
}

interface StreamConfigurationProperties {
  configuration: StreamConfiguration
  onConfigurationChange: (
    newConfiguration:
      | StreamConfiguration
      | ((configuration: StreamConfiguration) => StreamConfiguration),
  ) => void
  onStartClick: () => void
}

export const DEFAULT_STREAM_CONFIGURATION: StreamConfiguration = {
  amount: 3000n,
  maxPacketAmount: 1000n,
}

export default function StreamConfigurator({
  configuration,
  onConfigurationChange,
  onStartClick,
}: StreamConfigurationProperties) {
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
            value={String(configuration.maxPacketAmount)}
            onChange={(event) =>
              onConfigurationChange((configuration) => ({
                ...configuration,
                maxPacketAmount: BigInt(event.target.value),
              }))
            }
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
            value={String(configuration.amount)}
            onChange={(event) =>
              onConfigurationChange((configuration) => ({
                ...configuration,
                amount: BigInt(event.target.value),
              }))
            }
          />
        </div>
      </div>
      <Button onClick={() => onStartClick()}>Simulate</Button>
    </div>
  )
}
