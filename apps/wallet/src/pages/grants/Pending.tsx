/**
 * Grants that wait for the owner (spec/designs/periphery.md#6-owner-apps): an
 * app asked for access the owner decides. The wallet shows each as it comes,
 * or as the wallet connects, wherever the client sent the owner: a client of
 * a node without a hostname sends them to a front door, which serves no
 * wallet. Each leads to its consent page.
 *
 * Anyone can ask, so the band every page shows names no one: the name an app
 * gives is its own text, and only Connected apps and the consent page show
 * it, beside the app's wallet address.
 */

import type { JSX } from "@solidjs/web";
import type { PendingGrant } from "@dassie/rpc/proto";
import { type Accessor, createContext, For, Show, useContext } from "solid-js";
import { dateTime } from "@dassie/ui/format";
import { createLiveValue } from "@dassie/ui/live";
import { Link, useRouter } from "@dassie/ui/router";
import { Card } from "@dassie/ui/ui";
import { clientName, consentPath, pendingNotice } from "./grants.ts";

/** The grants that wait for the owner, live, from one stream for the shell. */
const PendingContext = createContext<Accessor<readonly PendingGrant[]>>();

/** Follows the grants that wait, once, for the band and Connected apps. */
export function PendingGrants(props: { children: JSX.Element }): JSX.Element {
  const live = createLiveValue(
    "subscribePendingGrants",
    () => ({}),
    (item) => item.grants,
  );
  const grants = () => live.value() ?? [];
  return <PendingContext value={grants}>{props.children}</PendingContext>;
}

/** The grants that wait, as the shell follows them. */
function usePending(): Accessor<readonly PendingGrant[]> {
  return useContext(PendingContext);
}

/**
 * A band under the frame while an app waits for the owner, on every page but
 * those that show the requests themselves.
 */
export function PendingNotice(): JSX.Element {
  const router = useRouter();
  const pending = usePending();
  const shown = () =>
    !router.path().startsWith("/consent/") && router.path() !== "/grants";
  const notice = () => (shown() ? pendingNotice(pending()) : undefined);
  return (
    <Show when={notice()}>
      {(notice) => (
        <p class="banner banner-ask" role="status" data-testid="pending-banner">
          <span>{notice().text}</span>
          <Link href={notice().href} testid="pending-review">Review</Link>
        </p>
      )}
    </Show>
  );
}

/** Connected apps' list of the requests that wait, each with its way in. */
export function PendingList(): JSX.Element {
  const pending = usePending();
  return (
    <Show when={pending().length > 0}>
      <Card title="Waiting for you" testid="grants-pending">
        <ul class="grants">
          <For each={pending()} keyed={(grant) => grant.id}>
            {(grant) => <Row grant={grant()} />}
          </For>
        </ul>
      </Card>
    </Show>
  );
}

function Row(props: { grant: PendingGrant }): JSX.Element {
  return (
    <li data-testid={`pending-${props.grant.id}`}>
      <div class="grant-head">
        <strong>
          {clientName(props.grant.client, props.grant.clientName)}
        </strong>
        <span class="muted">
          asked {dateTime(props.grant.createdAtUnixMs)}, lapses{" "}
          {dateTime(props.grant.expiresAtUnixMs)}
        </span>
      </div>
      <Show when={props.grant.clientName !== ""}>
        <p class="muted grant-client">{props.grant.client}</p>
      </Show>
      <Link href={consentPath(props.grant)} testid="pending-open">
        Review the request
      </Link>
    </li>
  );
}
