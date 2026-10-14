/** Waiting for a process to come up. */

const POLL_MS = 50;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Waits until a TCP port on loopback accepts connections, failing early if
 * `gone` settles first: the process that should open it exited.
 */
export async function waitForPort(
  port: number,
  timeoutMs = 15_000,
  gone?: Promise<unknown>,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let exited = false;
  gone?.then(() => exited = true, () => exited = true);
  for (;;) {
    try {
      (await Deno.connect({ hostname: "127.0.0.1", port })).close();
      return;
    } catch (error) {
      if (exited) throw new Error(`the process for port ${port} exited`);
      if (Date.now() > deadline) {
        throw new Error(`port ${port} did not open: ${error}`);
      }
      await sleep(POLL_MS);
    }
  }
}

/**
 * Waits until the daemon answers on its IPC socket. It binds its TCP ports
 * first and publishes the socket only once it answers, so an open port does
 * not mean the socket is there yet; and a socket file left by a daemon that
 * was killed is there but answers nobody.
 */
export async function waitForSocket(
  path: string,
  timeoutMs = 15_000,
  gone?: Promise<unknown>,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let exited = false;
  gone?.then(() => exited = true, () => exited = true);
  for (;;) {
    const found = await Deno.connect({ transport: "unix", path }).then(
      (conn) => {
        conn.close();
        return true;
      },
      () => false,
    );
    if (found) return;
    if (exited) throw new Error(`the process for ${path} exited`);
    if (Date.now() > deadline) throw new Error(`${path} did not appear`);
    await sleep(POLL_MS);
  }
}

/** Waits until `done` holds, polling every quarter second. */
export async function until(
  what: string,
  done: () => Promise<boolean>,
  timeoutMs = 90_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await done())) {
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
    await sleep(250);
  }
}
