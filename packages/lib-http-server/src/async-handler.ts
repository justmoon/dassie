import type { SetReturnType } from "type-fest"

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const asyncHandler = <THandler extends (...parameters: any[]) => any>(
  handler: SetReturnType<THandler, Promise<void>>
): THandler =>
  ((...parameters: Parameters<THandler>) => {
    const returnValue = handler(...parameters)
    const next = parameters[2] as (error?: unknown) => void
    Promise.resolve(returnValue).catch(next)
  }) as THandler
