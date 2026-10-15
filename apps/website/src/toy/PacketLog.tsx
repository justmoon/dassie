/**
 * The STREAM toy's packet log: each ILP Prepare, its answer once it
 * arrives, and, opened, the STREAM packets inside them, decrypted with the
 * connection's shared secret.
 */

import type { JSX } from "@solidjs/web";
import type { Stream } from "@dassie/identity-wasm/web/stream";
import { type Accessor, createMemo, createSignal, For, Show } from "solid-js";
import {
  decodeStream,
  decodeTooLarge,
  describeFrame,
  frameName,
  ilpErrorName,
  type StreamPacket,
} from "./describe.ts";
import { ADDRESSES, type PacketRecord, type Response } from "./simulation.ts";

interface Shared {
  version: Accessor<number>;
  /** When the run started, which packet times count from. */
  start: number;
  sharedSecret: Uint8Array;
  stream: Stream;
}

const seconds = (start: number, at: number) =>
  `+${((at - start) / 1000).toFixed(3)} s`;

export function PacketLog(
  props: Shared & { packets: Accessor<PacketRecord[]> },
): JSX.Element {
  return (
    <section aria-label="Packets">
      <div class="packet-ends">
        <div>
          <strong>Sender</strong>
          <span class="muted">{ADDRESSES.sender}</span>
        </div>
        <div>
          <strong>Receiver</strong>
          <span class="muted">{ADDRESSES.receiver}</span>
        </div>
      </div>
      <div class="packets" data-testid="packets">
        <For each={props.packets()}>
          {(record) => <Packet {...props} record={record} />}
        </For>
      </div>
    </section>
  );
}

function Packet(props: Shared & { record: PacketRecord }): JSX.Element {
  const [open, setOpen] = createSignal(false);
  // Records change in place when their answer arrives, once; the version
  // says when to look again, and the memo passes on only that change.
  const response = createMemo(() => (props.version(), props.record.response));
  const outcome = () => {
    const answer = response();
    if (!answer) return "pending";
    return answer.fulfilled ? "fulfilled" : "rejected";
  };
  return (
    <div
      class={`packet from-${props.record.from} ${outcome()}`}
      data-testid="packet"
      data-outcome={outcome()}
    >
      <button
        type="button"
        class="packet-head"
        data-testid="packet-head"
        aria-expanded={open() ? "true" : "false"}
        onClick={() => setOpen(!open())}
      >
        <span class="packet-arrow" aria-hidden="true">
          {props.record.from === "sender" ? "→" : "←"}
        </span>
        <span class="packet-outcome">
          {outcome() === "pending"
            ? "Prepared"
            : outcome() === "fulfilled"
            ? "Fulfilled"
            : "Rejected"}
        </span>
        <Show when={response() && !response()!.fulfilled}>
          <span class="packet-code" data-testid="packet-code">
            {response()!.code} {ilpErrorName(response()!.code)}
          </span>
        </Show>
        <span class="packet-amount">{String(props.record.amount)}</span>
        <span class="packet-time">
          {seconds(props.start, props.record.sentAt)}
        </span>
      </button>
      <Show when={open()}>
        <Details {...props} response={response} />
      </Show>
    </div>
  );
}

function Details(
  props: Shared & {
    record: PacketRecord;
    response: Accessor<Response | undefined>;
  },
): JSX.Element {
  const { record } = props;
  const request = decodeStream(props.stream, props.sharedSecret, record.data);
  const answer = createMemo(() => {
    const response = props.response();
    if (!response) return undefined;
    if (!response.fulfilled && response.code === "F08") {
      return {
        response,
        tooLarge: decodeTooLarge(props.stream, response.data),
      };
    }
    return {
      response,
      stream: decodeStream(props.stream, props.sharedSecret, response.data),
    };
  });
  return (
    <div class="packet-details">
      <h3>ILP Prepare</h3>
      <dl>
        <dt>Amount</dt>
        <dd>{String(record.amount)}</dd>
        <dt>Destination</dt>
        <dd>{record.destination}</dd>
        <dt>Sent</dt>
        <dd>{seconds(props.start, record.sentAt)}</dd>
        <dt>Expiry</dt>
        <dd>{new Date(record.expiresAt).toISOString()}</dd>
        <dt>Data</dt>
        <dd>{record.data.length} bytes</dd>
      </dl>
      <Show when={answer()}>
        {(current) => (
          <>
            <h3>ILP {current().response.fulfilled ? "Fulfill" : "Reject"}</h3>
            <dl>
              <Show when={!current().response.fulfilled}>
                <dt>Code</dt>
                <dd>
                  {current().response.code}{" "}
                  {ilpErrorName(current().response.code)}
                </dd>
                <dt>Message</dt>
                <dd>{current().response.message || "(none)"}</dd>
                <dt>Triggered by</dt>
                <dd>{current().response.triggeredBy || "(none)"}</dd>
              </Show>
              <dt>Arrived</dt>
              <dd>{seconds(props.start, current().response.at)}</dd>
              <dt>Data</dt>
              <dd>{current().response.data.length} bytes</dd>
            </dl>
          </>
        )}
      </Show>
      <h3>STREAM request</h3>
      <StreamDetails packet={request} response={false} />
      <Show when={answer()}>
        {(current) => {
          const { tooLarge, stream } = current() as {
            tooLarge?: ReturnType<typeof decodeTooLarge>;
            stream?: ReturnType<typeof decodeStream>;
          };
          if (tooLarge !== undefined) {
            return (
              <>
                <h3>F08 data</h3>
                {typeof tooLarge === "string"
                  ? <p class="error">{tooLarge}</p>
                  : (
                    <dl>
                      <dt>Received amount</dt>
                      <dd>{String(tooLarge.received)}</dd>
                      <dt>Maximum amount</dt>
                      <dd>{String(tooLarge.maximum)}</dd>
                    </dl>
                  )}
              </>
            );
          }
          return (
            <>
              <h3>STREAM response</h3>
              <StreamDetails packet={stream ?? "No data"} response />
            </>
          );
        }}
      </Show>
    </div>
  );
}

function StreamDetails(props: {
  packet: StreamPacket | string;
  response: boolean;
}): JSX.Element {
  const packet = props.packet;
  if (typeof packet === "string") return <p class="error">{packet}</p>;
  return (
    <dl>
      <dt>{props.response ? "Received amount" : "Least amount"}</dt>
      <dd>{String(packet.prepareAmount)}</dd>
      <dt>Sequence</dt>
      <dd>{String(packet.sequence)}</dd>
      <dt>Frames</dt>
      <dd>
        {packet.frames.length === 0
          ? <span class="muted">None</span>
          : (
            <ul class="frames">
              <For each={packet.frames}>
                {(frame) => (
                  <li>
                    <span class="frame-kind">{frameName(frame)}</span>
                    <span>{describeFrame(frame)}</span>
                  </li>
                )}
              </For>
            </ul>
          )}
      </dd>
    </dl>
  );
}
