import { DassieActorContext } from "../../base/types/dassie-base"

export interface TrpcContext {
  sig: DassieActorContext
  user: boolean
}
