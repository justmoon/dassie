import { useMemo } from "react"

import { parseAmountTooLargeData } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

interface AmountTooLargeResponseDetailsProperties {
  data: Uint8Array
}

export default function AmountTooLargeResponseDetails({
  data,
}: AmountTooLargeResponseDetailsProperties) {
  const parseResult = useMemo(() => {
    return parseAmountTooLargeData(data)
  }, [data])

  return (
    <div className="grid grid-cols-subgrid col-span-2">
      {isFailure(parseResult) ?
        <div className="text-red-300 col-span-2">{parseResult.message}</div>
      : <>
          <div className="text-muted-foreground">Received Amount</div>
          <div>{String(parseResult.receivedAmount)}</div>
          <div className="text-muted-foreground">Maximum Amount</div>
          <div>{String(parseResult.maximumAmount)}</div>
        </>
      }
    </div>
  )
}
