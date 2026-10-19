/**
 * The dev server's keys on a terminal: space opens the dashboard, `r`
 * restarts the nodes, `q` quits. The terminal is in raw mode, where ctrl+c is
 * a byte like any other, so that byte sends this process SIGINT, which the dev
 * server handles.
 *
 * @module
 */

import { spawn } from "node:child_process";
import process from "node:process";

export interface KeyHandlers {
  open(): void;
  restart(): void;
  quit(): void;
}

/** Listens for keys until closed; restores the terminal on close. */
export function listenKeys(handlers: KeyHandlers): { close(): void } {
  const stdin = process.stdin;
  stdin.setRawMode(true);
  const onData = (bytes: Uint8Array) => {
    for (const byte of bytes) {
      if (byte === 0x20) handlers.open();
      else if (byte === 0x72) handlers.restart(); // r
      else if (byte === 0x71) handlers.quit(); // q
      else if (byte === 0x03) process.kill(process.pid, "SIGINT"); // ctrl+c
    }
  };
  // The terminal going away ends the keys, nothing more.
  const onError = () => {};
  stdin.on("data", onData);
  stdin.on("error", onError);
  stdin.resume();
  let closed = false;
  return {
    close() {
      if (closed) return;
      closed = true;
      stdin.off("data", onData);
      stdin.off("error", onError);
      try {
        stdin.setRawMode(false);
      } catch {
        // Not a terminal any more.
      }
      // Stdin read no more, so that it holds nothing open.
      stdin.pause();
    },
  };
}

/** Opens `url` in the default browser. */
export function openBrowser(url: string): void {
  const command = process.platform === "darwin" ? "open" : "xdg-open";
  const child = spawn(command, [url], { stdio: "ignore" });
  child.on("error", () => console.log(`open ${url} in your browser`));
}
