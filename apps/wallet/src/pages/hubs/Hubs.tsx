/**
 * Settlement hubs (spec/areas/wallet.md#pages): every hub the node is in,
 * its account there, the holds toward and from its peers, the floor and
 * ceiling the node keeps it between, the move on its way, and moving funds
 * in and out, as the node reports them (spec/areas/hubs.md#in-the-node);
 * and each hub's payout once it ended or stalled
 * (spec/areas/hubs.md#payout-bundles). No stream carries hubs, so the page
 * asks again every few seconds while it shows (spec/areas/wallet.md#live-data).
 * A token hub its issuer froze says so, and since when
 * (spec/areas/hubs.md#a-frozen-hub). A withdrawal from a token hub that
 * the node holds back while it checks the escrow's XRP goes again by
 * itself, a few times, while the page shows. It draws in Kopje
 * (apps/website/brand/index.html): the value cube only beside the balance,
 * ember for action.
 */

import type { JSX } from "@solidjs/web";
import type {
  GetHubPayoutResponse,
  HubHold,
  HubStatus,
} from "@dassie/rpc/proto";
import {
  type Accessor,
  createSignal,
  createUniqueId,
  For,
  onCleanup,
  Show,
} from "solid-js";
import { RpcError } from "@dassie/rpc";
import { Amount } from "@dassie/ui/amount";
import {
  type Currency,
  parseAmount,
  parseFailureText,
} from "@dassie/ui/currency";
import { shortId } from "@dassie/ui/format";
import { useNodeData } from "@dassie/ui/node-data";
import { Link } from "@dassie/ui/router";
import { describe, useClient } from "@dassie/ui/rpc";
import { useToast } from "@dassie/ui/toast";
import {
  Alert,
  Badge,
  Card,
  ConfirmButton,
  CopyButton,
  ErrorText,
  Field,
  PageTitle,
  Spinner,
  ValueCube,
} from "@dassie/ui/ui";
import { schemeName } from "../schemes.ts";
import {
  afterWithdrawalFailed,
  automaticText,
  type CallFailure,
  GAS_CHECK_TEXT,
  givenUpText,
  holdRows,
  holdsTotal,
  HUB_POLL_MS,
  hubCurrency,
  hubIssuerText,
  hubStandingText,
  LATE_TEXT,
  moveText,
  PAYOUT_POLL_MS,
  payoutState,
  payoutText,
  validatorsText,
  withHub,
} from "./hubs.ts";

/** The hubs as the node last reported them, asked again every
 * {@linkcode HUB_POLL_MS}, one request at a time, while the page shows. */
interface PolledHubs {
  /** Undefined until the node first answered. */
  hubs: Accessor<readonly HubStatus[] | undefined>;
  /** Why the last ask failed, while it did. */
  error: Accessor<string | undefined>;
  /** Each hub's payout, by its ID, asked every {@linkcode PAYOUT_POLL_MS}. */
  payouts: Accessor<ReadonlyMap<string, GetHubPayoutResponse>>;
  /** When the hubs were last asked for, by the browser's clock. */
  now: Accessor<number>;
  /** Takes a hub's status an action answered with. */
  take(status: HubStatus): void;
}

function pollHubs(): PolledHubs {
  const client = useClient();
  const [hubs, setHubs] = createSignal<readonly HubStatus[]>();
  const [error, setError] = createSignal<string>();
  const [payouts, setPayouts] = createSignal<
    ReadonlyMap<string, GetHubPayoutResponse>
  >(new Map());
  const [now, setNow] = createSignal(Date.now());
  let asking = false;
  // How many answers of actions the page took: a poll that started before
  // the latest of them answers with the state from before it, and is
  // dropped.
  let taken = 0;
  let payoutsAskedMs = Number.NEGATIVE_INFINITY;
  const askPayouts = async (list: readonly HubStatus[]) => {
    const next = new Map<string, GetHubPayoutResponse>();
    for (const hub of list) {
      try {
        next.set(
          hub.hubId,
          await client.call("getHubPayout", { scheme: hub.scheme }),
        );
      } catch {
        // Keep what is known; the next round asks again.
        const known = payouts().get(hub.hubId);
        if (known) next.set(hub.hubId, known);
      }
    }
    setPayouts(next);
  };
  const ask = async () => {
    if (asking) return;
    asking = true;
    const takenBefore = taken;
    try {
      const answer = await client.call("getHubStatus", {});
      if (taken !== takenBefore) return;
      setHubs(answer.hubs);
      setError(undefined);
      setNow(Date.now());
      if (Date.now() - payoutsAskedMs >= PAYOUT_POLL_MS) {
        payoutsAskedMs = Date.now();
        await askPayouts(answer.hubs);
      }
    } catch (failure) {
      setError(describe(failure));
    } finally {
      asking = false;
    }
  };
  void ask();
  const timer = setInterval(() => void ask(), HUB_POLL_MS);
  onCleanup(() => clearInterval(timer));
  return {
    hubs,
    error,
    payouts,
    now,
    take: (status) => {
      taken += 1;
      setHubs(withHub(hubs() ?? [], status));
    },
  };
}

export function Hubs(): JSX.Element {
  const polled = pollHubs();
  return (
    <div class="page-wide stack" data-testid="page-hubs">
      <PageTitle>Settlement Hubs</PageTitle>
      <p class="muted hub-intro">
        Your node settles with its peers through settlement hubs: ledgers that a
        few validators keep together, backed by an escrow on the XRP Ledger.
        Here is your account in each hub, what you hold for your peers and what
        they hold for you.
      </p>
      <Show when={polled.error()}>
        {(error) => (
          <p class="muted live-note" role="status" data-testid="hubs-error">
            Not current: {error()}
          </p>
        )}
      </Show>
      <Show
        when={polled.hubs()}
        fallback={<Spinner label="Asking your node for its hubs…" />}
      >
        {(hubs) => (
          <Show when={hubs().length > 0} fallback={<NoHubs />}>
            <For each={hubs()} keyed={(hub) => hub.hubId}>
              {(hub) => <HubCard status={hub()} polled={polled} />}
            </For>
          </Show>
        )}
      </Show>
    </div>
  );
}

/** What the page says while the node is in no hub. */
function NoHubs(): JSX.Element {
  return (
    <Card testid="hubs-none">
      <p class="muted">
        Your node is in no settlement hub yet. It joins one when you add the
        hub's settlement scheme, on <Link href="/ledgers/create">Ledgers</Link>.
      </p>
    </Card>
  );
}

function HubCard(
  props: { status: HubStatus; polled: PolledHubs },
): JSX.Element {
  const data = useNodeData();
  const id = () => props.status.hubId;
  const currency = () => hubCurrency(props.status.scheme, data.ledgers() ?? []);
  return (
    <Card testid={`hub-${id()}`} class="hub">
      <div class="hub-head">
        <h2>{schemeName(props.status.scheme)}</h2>
        <Badge testid={`hub-validators-${id()}`}>
          {validatorsText(
            props.status.validators,
            props.status.validatorsAnswering,
          )}
        </Badge>
      </div>
      <p class="muted hub-id">
        Hub <code title={id()}>{shortId(id(), 16)}</code>{" "}
        <CopyButton text={id()} label="Copy ID" />
        <Show when={props.status.account !== undefined}>
          {" "}· your account {props.status.account}
        </Show>
      </p>
      <Show when={hubIssuerText(props.status, props.polled.now())}>
        {(text) => (
          <Alert
            title={props.status.frozenSinceUnixMs !== undefined
              ? "Frozen by its issuer"
              : "Held by its issuer"}
            tone="warning"
            testid={`hub-frozen-${id()}`}
          >
            {text()}
          </Alert>
        )}
      </Show>
      <Show when={hubStandingText(props.status)}>
        {(text) => <p data-testid={`hub-connection-${id()}`}>{text()}</p>}
      </Show>
      <Show
        when={currency()}
        fallback={
          <p class="muted" data-testid={`hub-no-ledger-${id()}`}>
            Your node lists no ledger for this hub's scheme yet, so its amounts
            cannot be shown.
          </p>
        }
      >
        {(currency) => (
          <HubAccount
            status={props.status}
            currency={currency()}
            polled={props.polled}
          />
        )}
      </Show>
      <Payout id={id()} payout={props.polled.payouts().get(id())} />
    </Card>
  );
}

function HubAccount(
  props: { status: HubStatus; currency: Currency; polled: PolledHubs },
): JSX.Element {
  const id = () => props.status.hubId;
  const amount = (value: bigint, testid: string) => (
    <Amount
      value={value}
      currency={props.currency}
      testid={`${testid}-${id()}`}
    />
  );
  return (
    <>
      <div class="hub-balance">
        <ValueCube />
        <div>
          <p class="hub-label">Yours to spend in the hub</p>
          <Amount
            value={props.status.balance}
            currency={props.currency}
            size="lg"
            testid={`hub-balance-${id()}`}
          />
        </div>
      </div>
      <dl class="facts hub-facts">
        <dt>Held back as the account's reserve</dt>
        <dd>{amount(props.status.reserve, "hub-reserve")}</dd>
        <dt>Deposited, all told</dt>
        <dd>{amount(props.status.deposited, "hub-deposited")}</dd>
        <dt>Withdrawn, with ledger fees</dt>
        <dd>{amount(props.status.withdrawn, "hub-withdrawn")}</dd>
        <Show when={props.status.returned > 0n}>
          <dt>Returned by withdrawals the escrow could not pay</dt>
          <dd>{amount(props.status.returned, "hub-returned")}</dd>
        </Show>
        <dt>Kept between, the rest going to your own account</dt>
        <dd>
          {amount(props.status.floor, "hub-floor")} and{" "}
          {amount(props.status.ceiling, "hub-ceiling")}
        </dd>
      </dl>
      <Holds
        title="You hold for your peers"
        empty="You hold nothing for a peer yet."
        holds={props.status.paying}
        currency={props.currency}
        testid={`hub-paying-${id()}`}
      />
      <Holds
        title="Your peers hold for you"
        empty="No peer holds anything for you yet."
        holds={props.status.taking}
        currency={props.currency}
        testid={`hub-taking-${id()}`}
      />
      <Moves
        status={props.status}
        currency={props.currency}
        polled={props.polled}
      />
    </>
  );
}

/** Holds toward peers, or from them: each peer's, and their total. */
function Holds(props: {
  title: string;
  empty: string;
  holds: readonly HubHold[];
  currency: Currency;
  testid: string;
}): JSX.Element {
  return (
    <section class="hub-holds" data-testid={props.testid}>
      <h3>{props.title}</h3>
      <Show
        when={props.holds.length > 0}
        fallback={<p class="muted">{props.empty}</p>}
      >
        <div class="table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Peer</th>
                <th scope="col">Held</th>
              </tr>
            </thead>
            <tbody>
              <For each={holdRows(props.holds)}>
                {(hold) => (
                  <tr data-testid={`${props.testid}-${hold.account}`}>
                    <td>
                      <Show
                        when={hold.peer}
                        fallback={
                          <span class="muted">account {hold.account}</span>
                        }
                      >
                        {(peer) => (
                          <Link href={`/network/${peer()}`}>
                            <code>{shortId(peer())}</code>
                          </Link>
                        )}
                      </Show>
                    </td>
                    <td>
                      <Amount value={hold.amount} currency={props.currency} />
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
            <tfoot>
              <tr>
                <th scope="row">All told</th>
                <td>
                  <Amount
                    value={holdsTotal(props.holds)}
                    currency={props.currency}
                    testid={`${props.testid}-total`}
                  />
                </td>
              </tr>
            </tfoot>
          </table>
        </div>
      </Show>
    </section>
  );
}

/** A failed call, as the page reads it. */
function failureOf(failure: unknown): CallFailure {
  return failure instanceof RpcError
    ? { code: failure.code, message: failure.message }
    : { message: describe(failure) };
}

/** The move on its way, the one given up, and moving funds. */
function Moves(
  props: { status: HubStatus; currency: Currency; polled: PolledHubs },
): JSX.Element {
  const client = useClient();
  const toast = useToast();
  const field = createUniqueId();
  const id = () => props.status.hubId;
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  // While the node checks a token hub's escrow's XRP, the withdrawal goes
  // again by itself: the timer that sends it, and a round that a cancel
  // or a new move ends, so an answer of an ended round starts no other.
  const [checking, setChecking] = createSignal(false);
  let retry: ReturnType<typeof setTimeout> | undefined;
  let round = 0;
  const stopRetrying = () => {
    round += 1;
    if (retry !== undefined) clearTimeout(retry);
    retry = undefined;
    setChecking(false);
  };
  onCleanup(stopRetrying);
  // A move the node took: the amount it was of goes, unless the owner
  // typed another since.
  const moved = (direction: "deposit" | "withdraw", clear = true) => {
    if (clear) setText("");
    toast({
      title: direction === "deposit"
        ? "Deposit on its way"
        : "Withdrawal on its way",
      description: "It shows here once the hub's validators agree on it.",
    });
  };
  const withdraw = async (
    request: { amount: bigint; scheme: string },
    retries: number,
  ) => {
    const mine = round;
    retry = undefined;
    setBusy(true);
    setError(undefined);
    try {
      const { status } = await client.call("hubWithdraw", request);
      if (status) props.polled.take(status);
      if (mine === round) setChecking(false);
      moved("withdraw", mine === round);
    } catch (failure) {
      if (mine !== round) return;
      const next = afterWithdrawalFailed(failureOf(failure), retries);
      if (next.kind === "retry") {
        setChecking(true);
        retry = setTimeout(
          () => void withdraw(request, retries + 1),
          next.afterMs,
        );
      } else {
        setChecking(false);
        setError(next.text);
      }
    } finally {
      setBusy(false);
    }
  };
  const act = async (what: () => Promise<{ status?: HubStatus }>) => {
    setBusy(true);
    setError(undefined);
    try {
      const { status } = await what();
      if (status) props.polled.take(status);
      return true;
    } catch (failure) {
      setError(describe(failure));
      return false;
    } finally {
      setBusy(false);
    }
  };
  const move = async (direction: "deposit" | "withdraw") => {
    const parsed = parseAmount(text(), props.currency.scale);
    if (!parsed.ok) {
      setError(parseFailureText(parsed.reason, props.currency.scale));
      return;
    }
    const request = { amount: parsed.value, scheme: props.status.scheme };
    stopRetrying();
    if (direction === "withdraw") {
      await withdraw(request, 0);
      return;
    }
    const done = await act(() => client.call("hubDeposit", request));
    if (done) moved("deposit");
  };
  return (
    <section class="hub-moves stack">
      <h3>Moving funds</h3>
      <p data-testid={`hub-automatic-text-${id()}`}>
        {automaticText(props.status, props.polled.now())}
      </p>
      <Show when={props.status.moving}>
        {(moving) => (
          <div data-testid={`hub-moving-${id()}`}>
            <p>{moveText(moving(), props.currency, props.polled.now())}</p>
            <Show when={moving().late}>
              <Alert title="Late" tone="warning" testid={`hub-late-${id()}`}>
                {LATE_TEXT}
              </Alert>
              <div class="actions">
                <ConfirmButton
                  label="Give up this move"
                  title="Give up this move?"
                  confirm="Give it up"
                  tone="danger"
                  testid={`hub-give-up-${id()}`}
                  disabled={busy()}
                  onConfirm={async () => {
                    await act(() =>
                      client.call("hubGiveUp", { scheme: props.status.scheme })
                    );
                  }}
                >
                  <p>
                    Your node forgets the move and books nothing. If it arrives
                    later anyway, your node books it then, once.
                  </p>
                </ConfirmButton>
              </div>
            </Show>
          </div>
        )}
      </Show>
      <Show when={props.status.givenUp}>
        {(given) => (
          <Alert title="A move you gave up" testid={`hub-given-up-${id()}`}>
            {givenUpText(given(), props.currency)}
          </Alert>
        )}
      </Show>
      <Show when={checking()}>
        <Alert
          title="Checking the hub's fees"
          testid={`hub-gas-check-${id()}`}
        >
          <p>{GAS_CHECK_TEXT}</p>
          <div class="actions">
            <button
              type="button"
              class="link"
              data-testid={`hub-gas-check-cancel-${id()}`}
              onClick={stopRetrying}
            >
              Don't send it again
            </button>
          </div>
        </Alert>
      </Show>
      {/* Enter moves nothing: the owner picks which way with a button. */}
      <form onSubmit={(event: SubmitEvent) => event.preventDefault()}>
        <Field
          label={`Amount, in ${props.currency.code}`}
          for={field}
          hint="A deposit comes from your own ledger of this asset; a withdrawal goes back to it. Either one pauses your node's own moves for a while."
        >
          <input
            id={field}
            inputmode="decimal"
            autocomplete="off"
            data-testid={`hub-amount-${id()}`}
            value={text()}
            onInput={(event) => {
              // A withdrawal waiting to go again is of the amount it was:
              // a new one is the owner's to send.
              if (checking()) stopRetrying();
              setText(event.currentTarget.value);
            }}
          />
        </Field>
        <div class="actions">
          <button
            type="button"
            data-testid={`hub-withdraw-${id()}`}
            disabled={busy()}
            onClick={() => void move("withdraw")}
          >
            Withdraw
          </button>
          <button
            type="button"
            class="primary"
            data-testid={`hub-deposit-${id()}`}
            disabled={busy()}
            onClick={() => void move("deposit")}
          >
            Deposit
          </button>
        </div>
      </form>
      <div class="actions">
        <button
          type="button"
          class="link"
          data-testid={`hub-automatic-${id()}`}
          disabled={busy()}
          onClick={() =>
            void act(() =>
              client.call("setHubAutomatic", {
                automatic: !props.status.automatic,
                scheme: props.status.scheme,
              })
            )}
        >
          {props.status.automatic
            ? "Stop your node's own moves"
            : "Let your node move funds by itself"}
        </button>
      </div>
      <ErrorText message={error()} testid={`hub-error-${id()}`} />
    </section>
  );
}

/** The hub's payout, once a validator signed one. */
function Payout(
  props: { id: string; payout: GetHubPayoutResponse | undefined },
): JSX.Element {
  const state = () => props.payout && payoutState(props.payout);
  const text = () => {
    const known = state();
    return known && payoutText(known);
  };
  const ready = () => {
    const known = state();
    return known?.kind === "ready" ? known : undefined;
  };
  return (
    <Show when={text()}>
      {(text) => (
        <section class="hub-payout" data-testid={`hub-payout-${props.id}`}>
          <h3>Payout</h3>
          <p>{text()}</p>
          <Show when={ready()}>
            {(ready) => (
              <div class="actions">
                <CopyButton
                  text={ready().transactions.join("\n")}
                  label="Copy the transactions"
                  testid={`hub-payout-copy-${props.id}`}
                />
              </div>
            )}
          </Show>
        </section>
      )}
    </Show>
  );
}
