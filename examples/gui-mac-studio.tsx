/** @jsxImportSource kiln */
import {
  createSignal,
  createMemo,
  runApp,
} from "kiln:gui";

// Declarative reusable Metric Card component
function MetricBadge(props: { title: string; value: () => string | number; color?: string }) {
  return (
    <card>
      <text
        variant="caption"
        color="secondary"
      >
        {props.title}
      </text>
      <text
        variant="title"
        color={props.color || "white"}
      >
        {props.value}
      </text>
    </card>
  );
}

function StudioApp() {
  const [workers, setWorkers] = createSignal(8);
  const [renderScale, setRenderScale] = createSignal(100);
  const [recording, setRecording] = createSignal(false);
  const [accelerated, setAccelerated] = createSignal(true);

  const statusText = createMemo(() => (recording() ? "● RECORDING" : "○ IDLE"));
  const heapText = createMemo(() => `${workers() * 64} MB Heap`);

  return (
    <window
      title="Kiln Native Studio - TSX & Taffy Engine"
      width={920}
      height={620}
      style={{ gap: 16 }}
    >
      {/* Header bar */}
      <hstack spacing={12}>
        <text variant="title">
          Kiln Native Studio
        </text>
        <spacer />
        <button
          variant={recording() ? "destructive" : "primary"}
          onPress={() => setRecording(!recording())}
        >
          {() => (recording() ? "Stop Recording" : "Start Profiling")}
        </button>
      </hstack>

      <text variant="caption" color="secondary">
        Declarative TSX • Direct AppKit retain bindings • Solid 2.0 reactivity • Taffy 0.14 native layout
      </text>

      {/* 3-Column Taffy Native Grid */}
      <grid
        columns="1fr 1fr 1fr"
        gap={12}
        style={{ width: "100%", height: 84 }}
      >
        <MetricBadge
          title="ACTIVE WORKERS"
          value={() => `${workers()} Workers`}
          color="blue"
        />
        <MetricBadge
          title="ALLOCATED HEAP"
          value={heapText}
          color="purple"
        />
        <MetricBadge
          title="ENGINE STATUS"
          value={statusText}
          color={recording() ? "red" : "green"}
        />
      </grid>

      {/* Performance & Concurrency Card */}
      <card>
        <hstack spacing={10}>
          <text variant="headline">
            ⚡ Compute Engine & Concurrency
          </text>
          <spacer />
          <text variant="headline" color="blue">
            {() => `${workers()} Workers`}
          </text>
        </hstack>

        <hstack spacing={10}>
          <slider
            min={1}
            max={32}
            value={workers()}
            onValueChange={(val: number) => setWorkers(Math.round(val))}
          />
          <spacer />
          <button onPress={() => setWorkers((w: number) => Math.max(1, w - 1))}>
            − 1
          </button>
          <button onPress={() => setWorkers(8)}>
            8 Default
          </button>
          <button onPress={() => setWorkers((w: number) => Math.min(32, w + 1))}>
            + 1
          </button>
        </hstack>
      </card>

      {/* Hardware Configuration Card */}
      <card>
        <hstack spacing={12}>
          <toggle
            checked={accelerated()}
            onChange={(checked: boolean) => setAccelerated(checked)}
          />
          <text variant="body">
            Metal CoreAnimation Hardware Blending
          </text>
          <spacer />
          <text variant="badge" color={accelerated() ? "green" : "secondary"}>
            {() => (accelerated() ? "🟢 ACCELERATED" : "⚪ CPU RENDER")}
          </text>
        </hstack>
      </card>

      {/* Spacer to push footer to bottom */}
      <spacer />

      {/* Footer bar */}
      <hstack spacing={12}>
        <text variant="caption" color="secondary">
          Ready • Zero Virtual DOM Overhead • Pure Native Retained Cocoa Objects
        </text>
        <spacer />
        <button
          variant="secondary"
          onPress={() => {
            setWorkers(4 + Math.floor(Math.random() * 24));
          }}
        >
          Simulate Pulse
        </button>
      </hstack>
    </window>
  );
}

runApp(<StudioApp />);
