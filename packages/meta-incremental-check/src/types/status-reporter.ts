export type StatusReporter = (
  packageName: string,
  status: "start" | "success" | "error",
) => void
