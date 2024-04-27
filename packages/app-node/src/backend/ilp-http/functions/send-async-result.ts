import { ilpHttp as logger } from "../../logger/instances"
import { ILP_OVER_HTTP_CONTENT_TYPE } from "../constants/content-type"

export interface IlpHttpAsyncRequestParameters {
  packet: Uint8Array
  url: string
  requestId: string
}

export interface IlpHttpAsyncResponseParameters {
  packet: Uint8Array
  callbackUrl: string
  requestId: string
}

async function sendAsyncResult({
  packet,
  callbackUrl,
  requestId,
}: IlpHttpAsyncResponseParameters) {
  logger.debug?.("send ilp-http result", { packet })
  await fetch(callbackUrl, {
    method: "POST",
    body: packet,
    headers: {
      accept: ILP_OVER_HTTP_CONTENT_TYPE,
      "content-type": ILP_OVER_HTTP_CONTENT_TYPE,
      "request-id": requestId,
    },
  })
}

export const SendAsyncResult = () => {
  return sendAsyncResult
}
