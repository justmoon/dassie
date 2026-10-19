/**
 * The STREAM toy (spec/areas/website.md#stream-toy): settings, then a run of
 * the simulation, with its packets and its log, played at the speed the
 * settings ask for.
 */

import type { JSX } from "@solidjs/web";
import { create } from "@bufbuild/protobuf";
import { LogLevel, type LogLine, LogLineSchema } from "@dassie/rpc/proto";
import { LogViewer } from "@dassie/ui/logs";
import type { Stream } from "@dassie/identity-wasm/web/stream";
import { createMemo, createSignal, onCleanup, Show } from "solid-js";
import {
  DEFAULT_FORM,
  type Form,
  MAX_DILATION,
  MIN_DILATION,
  type Parsed,
  parseForm,
} from "./form.ts";
import { PacketLog } from "./PacketLog.tsx";
import {
  type LogEntry,
  MAX_IN_FLIGHT,
  MAX_JITTER,
  MAX_LATENCY,
  Simulation,
} from "./simulation.ts";

/** A run: its simulation and how fast it plays. */
interface Run {
  simulation: Simulation;
  timeDilation: number;
}

export function Toy(props: { stream: Stream }): JSX.Element {
  const [form, setForm] = createSignal<Form>(DEFAULT_FORM);
  const [run, setRun] = createSignal<Run>();
  const start = (parsed: Parsed) => {
    // Whole seconds, so the log's clock starts at a round time.
    const now = Math.floor(Date.now() / 1000) * 1000;
    setRun({
      simulation: new Simulation(props.stream, parsed.settings, now),
      timeDilation: parsed.timeDilation,
    });
  };
  return (
    <Show
      keyed
      when={run()}
      fallback={
        <Settings
          form={form()}
          onChange={setForm}
          onStart={start}
        />
      }
    >
      {(current) => (
        <Simulator
          run={current}
          stream={props.stream}
          onBack={() => setRun(undefined)}
          onAgain={() => {
            const parsed = parseForm(form());
            if (typeof parsed !== "string") start(parsed);
          }}
        />
      )}
    </Show>
  );
}

function Field(props: {
  id: keyof Form;
  label: string;
  hint: string;
  form: Form;
  onChange: (form: Form) => void;
}): JSX.Element {
  return (
    <div class="field">
      <label for={`toy-${props.id}`}>{props.label}</label>
      <input
        id={`toy-${props.id}`}
        data-testid={`toy-${props.id}`}
        inputmode="decimal"
        autocomplete="off"
        value={props.form[props.id]}
        aria-describedby={`toy-${props.id}-hint`}
        onInput={(event) =>
          props.onChange({
            ...props.form,
            [props.id]: event.currentTarget.value,
          })}
      />
      <p class="hint muted" id={`toy-${props.id}-hint`}>{props.hint}</p>
    </div>
  );
}

function Settings(props: {
  form: Form;
  onChange: (form: Form) => void;
  onStart: (parsed: Parsed) => void;
}): JSX.Element {
  const [problem, setProblem] = createSignal<string>();
  const field = (id: keyof Form, label: string, hint: string) => (
    <Field
      id={id}
      label={label}
      hint={hint}
      form={props.form}
      onChange={props.onChange}
    />
  );
  return (
    <form
      novalidate
      onSubmit={(event) => {
        event.preventDefault();
        const parsed = parseForm(props.form);
        if (typeof parsed === "string") setProblem(parsed);
        else props.onStart(parsed);
      }}
    >
      <div class="toy-settings">
        <section class="card">
          <h2>The network</h2>
          {field(
            "maxPacketAmount",
            "Largest packet",
            "The connector refuses larger packets with F08 Amount Too Large.",
          )}
          {field(
            "latency",
            "Latency (ms)",
            `The median round trip to the receiver, up to ${MAX_LATENCY} ms.`,
          )}
          {field(
            "jitter",
            "Jitter",
            `How much round trips vary, from 0 to ${MAX_JITTER}: the σ of their log-normal distribution.`,
          )}
          {field(
            "maxPacketsInFlight",
            "Packets in flight",
            `The connector refuses packets beyond this many with T03 Connector Busy, up to ${MAX_IN_FLIGHT}.`,
          )}
        </section>
        <section class="card">
          <h2>The payment</h2>
          {field(
            "amount",
            "Amount",
            "What the sender sends, in the smallest unit of the asset.",
          )}
          <h2>The simulation</h2>
          {field(
            "timeDilation",
            "Time dilation",
            `How fast it plays, from ${MIN_DILATION} to ${MAX_DILATION}: 1 is real time, 0.1 ten times slower.`,
          )}
          {field(
            "seed",
            "Seed",
            "Picks the round trips. The same settings and seed replay the same run.",
          )}
        </section>
      </div>
      <Show when={problem()}>
        <p class="error" role="alert" data-testid="toy-problem">{problem()}</p>
      </Show>
      <div class="actions">
        <button
          type="button"
          onClick={() => {
            setProblem(undefined);
            props.onChange(DEFAULT_FORM);
          }}
        >
          Reset
        </button>
        <button type="submit" class="primary" data-testid="toy-simulate">
          Simulate
        </button>
      </div>
    </form>
  );
}

const LEVELS: Record<LogEntry["level"], LogLevel> = {
  debug: LogLevel.DEBUG,
  info: LogLevel.INFO,
  warn: LogLevel.WARN,
};

function Simulator(props: {
  run: Run;
  stream: Stream;
  onBack: () => void;
  onAgain: () => void;
}): JSX.Element {
  const simulation = props.run.simulation;
  const { settings } = simulation;
  // The simulation changes outside Solid; each change bumps the version,
  // which everything that shows it reads.
  const [version, setVersion] = createSignal(0);
  const changed = () => setVersion((value) => value + 1);
  const [paused, setPaused] = createSignal(false);
  /** Runs `action` on the simulation; an error there stops the run. */
  const act = (action: () => void) => {
    try {
      action();
    } catch (error) {
      simulation.fail(error);
    }
    changed();
  };

  // While the run plays, each frame advances it by the time since the last
  // one, dilated. The loop ends when the run is paused or over.
  let frame: number | undefined;
  let last = 0;
  const tick = (now: number) => {
    // A tab in the background gets no frames; the run waits for it.
    const elapsed = Math.min(now - last, 250);
    last = now;
    act(() =>
      simulation.advanceTo(simulation.now + elapsed * props.run.timeDilation)
    );
    frame = simulation.status.state === "running"
      ? requestAnimationFrame(tick)
      : undefined;
  };
  const play = () => {
    if (frame !== undefined || simulation.status.state !== "running") return;
    last = performance.now();
    frame = requestAnimationFrame(tick);
  };
  const halt = () => {
    if (frame !== undefined) cancelAnimationFrame(frame);
    frame = undefined;
  };
  play();
  onCleanup(() => {
    halt();
    simulation.dispose();
  });

  const pause = (value: boolean) => {
    setPaused(value);
    if (value) halt();
    else play();
  };
  const running = () => (version(), simulation.status.state === "running");
  const packets = createMemo(() => (version(), simulation.packets.slice()));
  const converted: LogLine[] = [];
  const lines = createMemo(() => {
    version();
    for (const entry of simulation.logs.slice(converted.length)) {
      converted.push(create(LogLineSchema, {
        index: BigInt(converted.length + 1),
        timeUnixMs: BigInt(Math.round(entry.at)),
        level: LEVELS[entry.level],
        target: entry.source,
        message: entry.message,
        fields: entry.fields.map(([name, value]) => ({ name, value })),
      }));
    }
    return converted.slice();
  });
  const elapsed = () => (version(), (simulation.now - simulation.start) / 1000);
  const delivered = () => (version(), simulation.delivered);
  const status = () => {
    version();
    const status = simulation.status;
    if (status.state === "done") {
      return status.delivered < settings.amount
        ? "Ended before delivering everything"
        : "Done";
    }
    if (status.state === "stopped") return `Stopped: ${status.reason}`;
    return paused() ? "Paused" : "Running";
  };
  const progress = () => Number((delivered() * 1000n) / settings.amount) / 10;

  return (
    <div class="stack">
      <div class="toy-controls">
        <div class="buttons">
          <button type="button" data-testid="toy-back" onClick={props.onBack}>
            Back
          </button>
          <Show
            when={paused()}
            fallback={
              <button
                type="button"
                data-testid="toy-pause"
                disabled={!running()}
                onClick={() => pause(true)}
              >
                Pause
              </button>
            }
          >
            <button
              type="button"
              data-testid="toy-play"
              disabled={!running()}
              onClick={() => pause(false)}
            >
              Play
            </button>
          </Show>
          <button
            type="button"
            data-testid="toy-step"
            disabled={!running()}
            title="Run the next event"
            onClick={() => {
              pause(true);
              act(() => simulation.step());
            }}
          >
            Step
          </button>
          <button
            type="button"
            data-testid="toy-finish"
            disabled={!running()}
            title="Run to the end at once"
            onClick={() => {
              halt();
              act(() => simulation.advanceTo(Infinity, 1_000_000));
            }}
          >
            Finish
          </button>
          <Show when={!running()}>
            <button
              type="button"
              class="primary"
              data-testid="toy-again"
              onClick={props.onAgain}
            >
              Run again
            </button>
          </Show>
        </div>
        <div class="toy-status" aria-live="polite">
          <span data-testid="toy-status">{status()}</span>
          <span>+{elapsed().toFixed(3)} s</span>
          <span data-testid="toy-delivered">
            {String(delivered())} of {String(settings.amount)} delivered
          </span>
          <span>{packets().length} packets</span>
        </div>
        <div
          class="toy-progress"
          role="progressbar"
          aria-label="Delivered"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={progress()}
        >
          <div class="toy-progress-bar" style={{ width: `${progress()}%` }} />
        </div>
      </div>
      <PacketLog
        packets={packets}
        version={version}
        start={simulation.start}
        sharedSecret={simulation.sharedSecret}
        stream={props.stream}
      />
      <h2>Log</h2>
      <LogViewer lines={lines} testid="toy-log" />
    </div>
  );
}
