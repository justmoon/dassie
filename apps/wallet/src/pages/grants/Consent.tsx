/**
 * The consent page (D9): an app asks to act for the owner, for example to
 * send payments up to a limit. The owner reads who asks and what for, and
 * approves or denies; the wallet then sends them back to the app, which
 * learns the answer from the node.
 */

import type { JSX } from "@solidjs/web";
import type { Grant } from "@dassie/rpc/proto";
import { createSignal, For, Show } from "solid-js";
import { describe, useClient } from "@dassie/ui/rpc";
import { type RouteProps, useSearchParam } from "@dassie/ui/router";
import { Alert, Card, ErrorText, PageTitle, Spinner } from "@dassie/ui/ui";
import { accessText, clientName } from "./grants.ts";

type Page =
  | { kind: "loading" }
  | { kind: "asking"; grant: Grant }
  | { kind: "gone"; message: string }
  | { kind: "decided"; approved: boolean };

export function Consent(props: RouteProps): JSX.Element {
  const client = useClient();
  const key = useSearchParam("key");
  const [page, setPage] = createSignal<Page>({ kind: "loading" });
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const load = async () => {
    try {
      const response = await client.call("getGrantRequest", {
        id: props.params.id ?? "",
        key: key() ?? "",
      });
      if (response.grant) setPage({ kind: "asking", grant: response.grant });
    } catch (failure) {
      setPage({ kind: "gone", message: describe(failure) });
    }
  };
  void load();
  const decide = async (approve: boolean) => {
    if (pending()) return;
    setPending(true);
    setError(undefined);
    try {
      const response = await client.call("decideGrant", {
        id: props.params.id ?? "",
        key: key() ?? "",
        approve,
      });
      setPage({ kind: "decided", approved: approve });
      // Back to the app, which learns the answer from the node.
      if (response.finishUrl !== "") location.assign(response.finishUrl);
    } catch (failure) {
      setError(describe(failure));
    }
    setPending(false);
  };
  return (
    <div class="page stack" data-testid="page-consent">
      <PageTitle>An App Asks for Access</PageTitle>
      <Show when={page().kind === "loading"}>
        <Spinner label="Loading the request…" />
      </Show>
      <Show when={pageGone(page())}>
        {(message) => (
          <Alert title="This request is no longer waiting" tone="warning">
            <p data-testid="consent-gone">
              It was decided already, or lapsed after 10 minutes ({message()}).
              Start again from the app.
            </p>
          </Alert>
        )}
      </Show>
      <Show when={pageGrant(page())}>
        {(grant) => (
          <Request
            grant={grant()}
            pending={pending()}
            onDecide={(approve) => void decide(approve)}
          />
        )}
      </Show>
      <Show when={pageDecided(page())}>
        {(approved) => (
          <Alert title={approved() ? "Approved" : "Denied"}>
            <p data-testid="consent-decided">
              {approved()
                ? "The app may now do what it asked. You can revoke it under Connected Apps."
                : "The app gets nothing."} You can close this page.
            </p>
          </Alert>
        )}
      </Show>
      <ErrorText message={error()} />
    </div>
  );
}

const pageGone = (page: Page) =>
  page.kind === "gone" ? page.message : undefined;
const pageGrant = (page: Page) =>
  page.kind === "asking" ? page.grant : undefined;
const pageDecided = (page: Page) =>
  page.kind === "decided" ? page.approved : undefined;

/** Who asks, what for, and the two answers. */
function Request(props: {
  grant: Grant;
  pending: boolean;
  onDecide: (approve: boolean) => void;
}): JSX.Element {
  const name = () => clientName(props.grant.client, props.grant.clientName);
  return (
    <Card testid="consent-request">
      <p>
        <strong data-testid="consent-client">{name()}</strong>
        <Show when={props.grant.clientName !== ""}>
          {" "}
          <span class="muted">({props.grant.client})</span>
        </Show>{" "}
        asks to:
      </p>
      <ul class="consent-access">
        <For each={props.grant.access}>
          {(access) => (
            <li data-testid="consent-access">{accessText(access)}</li>
          )}
        </For>
        <Show when={props.grant.subject !== ""}>
          <li>Learn that {props.grant.subject} is yours</li>
        </Show>
      </ul>
      <p class="muted">
        Only approve apps you trust. You can revoke access at any time.
      </p>
      <div class="actions">
        <button
          type="button"
          data-testid="consent-deny"
          disabled={props.pending}
          onClick={() => props.onDecide(false)}
        >
          Deny
        </button>
        <button
          type="button"
          class="primary"
          data-testid="consent-approve"
          disabled={props.pending}
          onClick={() => props.onDecide(true)}
        >
          Approve
        </button>
      </div>
    </Card>
  );
}
