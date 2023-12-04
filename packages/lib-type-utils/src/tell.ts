/**
 * Call an asynchronous function and log any errors that occur.
 *
 * @remarks
 *
 * Often, we want to call out to an asynchronous function, but we don't care about the result. We may still want to log
 * any errors that occur. This function makes it easy to start off some asynchronous work safely and ignore the outcome.
 *
 * @param callback - A callback that invokes the asynchronous function and returns a promise.
 */
export const tell = (callback: () => PromiseLike<unknown>) => {
  try {
    callback().then(undefined, (error: unknown) => {
      console.error("error while executing asynchronous action", { error })
    })
  } catch (error: unknown) {
    console.error("error while executing asynchronous action", { error })
  }
}
