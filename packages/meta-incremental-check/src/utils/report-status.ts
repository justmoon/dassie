import { parentPort } from "node:worker_threads"

type Status = "start" | "success" | "error"
export type ProgressMessage =
  | ["print", message: string]
  | ["status", packageName: string, status: Status]

function postProgress(message: ProgressMessage) {
  parentPort!.postMessage(message)
}

export function printToConsole(message: string) {
  postProgress(["print", message])
}

export function reportPackageStatus(packageName: string, status: Status) {
  postProgress(["status", packageName, status])
}
