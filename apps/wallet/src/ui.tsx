import type { JSX } from "@solidjs/web";

/** Every screen of the skeleton is one centered card with a title. */
export function Card(props: {
  title: string;
  testid?: string;
  children?: JSX.Element;
  footer?: JSX.Element;
}): JSX.Element {
  return (
    <main class="page">
      <p class="brand">Dassie Wallet</p>
      <section class="card" data-testid={props.testid}>
        <h1>{props.title}</h1>
        {props.children}
      </section>
      {props.footer}
    </main>
  );
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
