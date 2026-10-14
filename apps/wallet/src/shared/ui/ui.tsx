/**
 * The wallet's building blocks. Every style comes from a class (the page's
 * CSP allows no inline style), and every control has a label.
 */

import type { JSX } from "@solidjs/web";
import {
  createSignal,
  createUniqueId,
  For,
  type ParentProps,
  Show,
  untrack,
} from "solid-js";
import type { LiveStatus } from "../live/subscription.ts";

/** A section of a page, with its title. */
export function Card(
  props: ParentProps<{ title?: string; testid?: string; class?: string }>,
): JSX.Element {
  return (
    <section class={`card ${props.class ?? ""}`} data-testid={props.testid}>
      <Show when={props.title}>
        <h2>{props.title}</h2>
      </Show>
      {props.children}
    </section>
  );
}

/** A page's heading; the router focuses it on navigation. */
export function PageTitle(props: ParentProps): JSX.Element {
  return <h1 tabindex="-1">{props.children}</h1>;
}

/** Shows that something is under way; for screen readers too. */
export function Spinner(props: { label: string }): JSX.Element {
  return (
    <p class="pending" role="status">
      <span class="spinner" aria-hidden="true" />
      {props.label}
    </p>
  );
}

/** A note that stands out: information, a warning, or danger. */
export function Alert(
  props: ParentProps<{
    title: string;
    tone?: "info" | "warning" | "danger";
    testid?: string;
  }>,
): JSX.Element {
  return (
    <div
      class={`alert alert-${props.tone ?? "info"}`}
      role={props.tone === "danger" ? "alert" : "note"}
      data-testid={props.testid}
    >
      <strong>{props.title}</strong>
      <div>{props.children}</div>
    </div>
  );
}

/** A small label, good or bad. */
export function Badge(
  props: ParentProps<{ tone?: "good" | "bad" | "plain"; testid?: string }>,
): JSX.Element {
  return (
    <span
      class={`badge badge-${props.tone ?? "plain"}`}
      data-testid={props.testid}
    >
      {props.children}
    </span>
  );
}

/** An error, read out when it appears. */
export function ErrorText(
  props: { message?: string; testid?: string },
): JSX.Element {
  return (
    <Show when={props.message}>
      <p class="error" role="alert" data-testid={props.testid}>
        {props.message}
      </p>
    </Show>
  );
}

/** A page that failed, with a way to try again. */
export function PageError(
  props: { error: unknown; reset: () => void },
): JSX.Element {
  const message = () =>
    props.error instanceof Error ? props.error.message : String(props.error);
  return (
    <div class="page-narrow" data-testid="page-error">
      <Alert title="This page could not be shown" tone="danger">
        {message()}
      </Alert>
      <div class="actions">
        <button type="button" class="primary" onClick={() => props.reset()}>
          Try again
        </button>
      </div>
    </div>
  );
}

/** Where a live view stands, when it is not live. */
export function LiveNote(props: { status: LiveStatus }): JSX.Element {
  const text = () => {
    const status = props.status;
    switch (status.kind) {
      case "connecting":
        return "Connecting…";
      case "retrying":
        return `Reconnecting (attempt ${status.attempt}): ${status.message}`;
      case "failed":
        return `Not available: ${status.message}`;
      case "live":
        return undefined;
    }
  };
  return (
    <Show when={text()}>
      <p class="muted live-note" role="status" data-testid="live-status">
        {text()}
      </p>
    </Show>
  );
}

/** A checklist item: met, not met, or unknown. */
export interface CheckItem {
  id: string;
  label: string;
  state: "valid" | "invalid" | "unknown";
}

/** A list of prerequisites, each marked. */
export function Checklist(props: { items: readonly CheckItem[] }): JSX.Element {
  const mark = { valid: "✓", invalid: "✗", unknown: "?" } as const;
  return (
    <ul class="checklist">
      <For each={props.items}>
        {(item) => (
          <li
            class={`check-${item.state}`}
            data-testid={`prereq-${item.id}`}
            data-state={item.state}
          >
            <span aria-hidden="true">{mark[item.state]}</span> {item.label}
          </li>
        )}
      </For>
    </ul>
  );
}

/** Tabs: a list of buttons, and the panel of the selected one. */
export function Tabs(props: {
  label: string;
  tabs: readonly { id: string; title: string; panel: () => JSX.Element }[];
  selected: string;
  onSelect: (id: string) => void;
}): JSX.Element {
  const base = createUniqueId();
  const current = () =>
    props.tabs.find((tab) => tab.id === props.selected) ?? props.tabs[0];
  const keys = (event: KeyboardEvent) => {
    const index = props.tabs.findIndex((tab) => tab.id === current()?.id);
    const step = event.key === "ArrowRight"
      ? 1
      : event.key === "ArrowLeft"
      ? -1
      : 0;
    if (step === 0) return;
    const next =
      props.tabs[(index + step + props.tabs.length) % props.tabs.length];
    props.onSelect(next.id);
    document.getElementById(`${base}-${next.id}`)?.focus();
  };
  return (
    <div class="tabs">
      <div role="tablist" aria-label={props.label} onKeyDown={keys}>
        <For each={props.tabs} keyed={(tab) => tab.id}>
          {(tab) => (
            <button
              type="button"
              role="tab"
              id={`${base}-${tab().id}`}
              data-testid={`tab-${tab().id}`}
              aria-selected={current()?.id === tab().id ? "true" : "false"}
              tabindex={current()?.id === tab().id ? 0 : -1}
              onClick={() => props.onSelect(tab().id)}
            >
              {tab().title}
            </button>
          )}
        </For>
      </div>
      <div role="tabpanel" aria-labelledby={`${base}-${current()?.id}`}>
        {
          /* The tabs are made again as data changes; the panel only when
            another tab is chosen, so what it shows stays. */
        }
        <Show when={current()?.id} keyed>
          {(id) =>
            untrack(() => props.tabs.find((tab) => tab.id === id)?.panel())}
        </Show>
      </div>
    </div>
  );
}

/**
 * A button that asks before it acts, in a modal dialog: the browser's own
 * `<dialog>` keeps focus inside and closes on Escape.
 */
export function ConfirmButton(props: {
  label: string;
  title: string;
  confirm: string;
  tone?: "danger";
  testid: string;
  disabled?: boolean;
  children: JSX.Element;
  onConfirm: () => void | Promise<void>;
}): JSX.Element {
  let dialog: HTMLDialogElement | undefined;
  const [pending, setPending] = createSignal(false);
  return (
    <>
      <button
        type="button"
        class={props.tone === "danger" ? "danger" : ""}
        data-testid={props.testid}
        disabled={props.disabled || pending()}
        onClick={() => dialog?.showModal()}
      >
        {props.label}
      </button>
      <dialog ref={dialog} data-testid={`${props.testid}-dialog`}>
        <h2>{props.title}</h2>
        {props.children}
        <div class="actions">
          <button type="button" onClick={() => dialog?.close()}>Cancel</button>
          <button
            type="button"
            class={props.tone === "danger" ? "danger" : "primary"}
            data-testid={`${props.testid}-confirm`}
            disabled={pending()}
            onClick={async () => {
              if (pending()) return;
              setPending(true);
              dialog?.close();
              try {
                await props.onConfirm();
              } finally {
                setPending(false);
              }
            }}
          >
            {props.confirm}
          </button>
        </div>
      </dialog>
    </>
  );
}

/** Copies `text`; says whether it worked. */
export function CopyButton(props: {
  text: string;
  label?: string;
  testid?: string;
  onCopied?: (ok: boolean) => void;
}): JSX.Element {
  const [copied, setCopied] = createSignal(false);
  return (
    <button
      type="button"
      data-testid={props.testid}
      onClick={async () => {
        let ok = true;
        try {
          await navigator.clipboard.writeText(props.text);
        } catch {
          ok = false;
        }
        props.onCopied?.(ok);
        if (ok) {
          setCopied(true);
          setTimeout(() => setCopied(false), 1_000);
        }
      }}
    >
      {copied() ? "Copied ✓" : props.label ?? "Copy"}
    </button>
  );
}

/** A secret, shown as dots until revealed. */
export function Masked(props: { text: string; testid?: string }): JSX.Element {
  const [shown, setShown] = createSignal(false);
  const masked = () =>
    "•".repeat(Math.max(0, props.text.length - 4)) + props.text.slice(-4);
  return (
    <span class="masked">
      <code data-testid={props.testid} data-revealed={shown()}>
        {shown() ? props.text : masked()}
      </code>
      <button
        type="button"
        class="link"
        data-testid={props.testid ? `${props.testid}-reveal` : undefined}
        aria-pressed={shown() ? "true" : "false"}
        onClick={() => setShown(!shown())}
      >
        {shown() ? "Hide" : "Show"}
      </button>
    </span>
  );
}

/** A labelled field around an input. */
export function Field(
  props: ParentProps<{ label: string; for: string; hint?: string }>,
): JSX.Element {
  return (
    <div class="field">
      <label for={props.for}>{props.label}</label>
      {props.children}
      <Show when={props.hint}>
        <p class="muted hint">{props.hint}</p>
      </Show>
    </div>
  );
}
