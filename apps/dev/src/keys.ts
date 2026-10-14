/**
 * The dev server's keys on a terminal (spec 6 §6.7): space opens the
 * dashboard, `r` restarts the nodes, `q` quits. The terminal stays in
 * cbreak mode, so ctrl+c still sends SIGINT, which the dev server handles.
 *
 * @module
 */

export interface KeyHandlers {
  open(): void;
  restart(): void;
  quit(): void;
}

/** Listens for keys until closed; restores the terminal on close. */
export function listenKeys(handlers: KeyHandlers): { close(): void } {
  Deno.stdin.setRaw(true, { cbreak: true });
  const reader = Deno.stdin.readable.getReader();
  let closed = false;
  (async () => {
    while (!closed) {
      const { value, done } = await reader.read();
      if (done) break;
      for (const byte of value) {
        if (byte === 0x20) handlers.open();
        else if (byte === 0x72) handlers.restart(); // r
        else if (byte === 0x71) handlers.quit(); // q
      }
    }
  })().catch(() => {
    // The terminal went away.
  });
  return {
    close() {
      if (closed) return;
      closed = true;
      reader.cancel().catch(() => {});
      try {
        Deno.stdin.setRaw(false);
      } catch {
        // Not a terminal any more.
      }
    },
  };
}

/** Opens `url` in the default browser. */
export function openBrowser(url: string): void {
  const command = Deno.build.os === "darwin" ? "open" : "xdg-open";
  try {
    new Deno.Command(command, {
      args: [url],
      stdin: "null",
      stdout: "null",
      stderr: "null",
    }).spawn().status.catch(() => {});
  } catch {
    console.log(`open ${url} in your browser`);
  }
}
