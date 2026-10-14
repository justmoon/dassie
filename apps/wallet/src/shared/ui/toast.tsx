/** Short notices in a corner, read out to screen readers. */

import type { JSX } from "@solidjs/web";
import {
  createContext,
  createSignal,
  For,
  type ParentProps,
  useContext,
} from "solid-js";

/** A notice. */
export interface Toast {
  title: string;
  description?: string;
  tone?: "default" | "destructive";
}

const MAX_TOASTS = 5;
const SHOWN_MS = 5_000;

const ToastContext = createContext<(toast: Toast) => void>();

/** Shows a notice. */
export function useToast(): (toast: Toast) => void {
  return useContext(ToastContext);
}

/** Holds the notices of its subtree. */
export function Toaster(props: ParentProps): JSX.Element {
  let next = 0;
  const [toasts, setToasts] = createSignal<(Toast & { id: number })[]>([]);
  const dismiss = (id: number) =>
    setToasts((all) => all.filter((t) => t.id !== id));
  const show = (toast: Toast) => {
    const id = next++;
    setToasts((all) => [...all, { ...toast, id }].slice(-MAX_TOASTS));
    setTimeout(() => dismiss(id), SHOWN_MS);
  };
  return (
    <ToastContext value={show}>
      {props.children}
      <div class="toasts" role="status" aria-live="polite">
        <For each={toasts()}>
          {(toast) => (
            <div
              class={`toast toast-${toast.tone ?? "default"}`}
              data-testid="toast"
            >
              <strong data-testid="toast-title">{toast.title}</strong>
              {toast.description ? <p>{toast.description}</p> : null}
              <button
                type="button"
                class="link"
                aria-label="Dismiss"
                data-testid="toast-close"
                onClick={() => dismiss(toast.id)}
              >
                ✕
              </button>
            </div>
          )}
        </For>
      </div>
    </ToastContext>
  );
}
