import { type IlpResponsePacket, IlpType } from "@dassie/lib-protocol-ilp"

interface IlpResponseDetailsProperties {
  response: IlpResponsePacket
}

export default function IlpResponseDetails({
  response,
}: IlpResponseDetailsProperties) {
  return (
    <div className="grid grid-cols-subgrid col-span-2">
      {response.type === IlpType.Reject && (
        <>
          <div className="text-muted-foreground">Message</div>
          <div>{response.data.message}</div>
          <div className="text-muted-foreground">Triggered By</div>
          <div>{response.data.triggeredBy}</div>
        </>
      )}
      <div className="text-muted-foreground">Data</div>
      <div>{response.data.data.length} bytes</div>
    </div>
  )
}
