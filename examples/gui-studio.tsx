/** @jsxImportSource kiln:gui */
// A tour of kiln:gui: a sidebar layout, grid of cards, every control, components with context,
// async data with a loading state, dialogs, the clipboard and a menu.
//
//   kiln run --native examples/gui-studio.tsx
import {
  app,
  clipboard,
  createContext,
  createMemo,
  createSignal,
  dialog,
  For,
  isPending,
  notify,
  runApp,
  Show,
  useContext,
} from "kiln:gui";

const Accent = createContext("blue");

function Metric(props: { title: string; value: () => string; detail?: () => string }) {
  const accent = useContext(Accent);
  return (
    <card gap={4}>
      <text variant="caption" color="secondary">
        {props.title.toUpperCase()}
      </text>
      <text variant="title" color={accent}>
        {props.value}
      </text>
      <Show when={() => props.detail}>
        <text variant="footnote" color="secondary" lines={1}>
          {() => (props.detail ? props.detail() : "")}
        </text>
      </Show>
    </card>
  );
}

const PAGES = ["Overview", "Controls", "Notes"];
const ICONS = ["gauge.with.dots.needle.33percent", "slider.horizontal.3", "note.text"];

function Studio() {
  const [page, setPage] = createSignal(0);
  const [workers, setWorkers] = createSignal(8);
  const [quality, setQuality] = createSignal(1);
  const [recording, setRecording] = createSignal(false);
  const [notes, setNotes] = createSignal("Notes are plain text.\nEdit freely.");
  const [region, setRegion] = createSignal(0);

  // An async derivation: the UI keeps the last value and shows a spinner while it waits.
  const throughput = createMemo(
    () => {
      const n = workers();
      return new Promise<string>((resolve) => setTimeout(() => resolve(`${(n * 1.7).toFixed(1)}k req/s`), 250));
    },
    "…",
  );
  const heap = createMemo(() => `${workers() * 64} MB`);
  const words = createMemo(() => notes().split(/\s+/).filter(Boolean).length);

  app.setMenu([
    {
      label: "Studio",
      items: [
        { label: "Overview", accelerator: "CmdOrCtrl+1", onClick: () => setPage(0) },
        { label: "Controls", accelerator: "CmdOrCtrl+2", onClick: () => setPage(1) },
        { label: "Notes", accelerator: "CmdOrCtrl+3", onClick: () => setPage(2) },
        "separator",
        { label: "Toggle Recording", accelerator: "CmdOrCtrl+R", onClick: () => setRecording((r) => !r) },
      ],
    },
  ]);

  return (
    <window title="kiln Studio" width={900} height={600} minWidth={640} minHeight={420} padding={0} gap={0} material="sidebar">
      <hstack grow align="stretch" gap={0}>
        {/* Sidebar */}
        <vstack width={190} padding={[16, 12]} gap={4}>
          <text variant="headline" color="secondary" margin={{ left: 6, bottom: 6 }}>
            Studio
          </text>
          <For each={PAGES}>
            {(name: string, index: () => number) => (
              <hstack
                padding={[6, 8]}
                cornerRadius={6}
                background={() => (page() === index() ? "selection" : "clear")}
                onPress={() => setPage(index())}
              >
                <image symbol={ICONS[index()]} size={14} color={() => (page() === index() ? "white" : "secondary")} />
                <text color={() => (page() === index() ? "white" : "label")}>{name}</text>
              </hstack>
            )}
          </For>
          <spacer />
          <hstack padding={[0, 6]}>
            <Show when={recording} fallback={<image symbol="circle" size={10} color="secondary" />}>
              <image symbol="record.circle.fill" size={10} color="red" />
            </Show>
            <text variant="footnote" color="secondary">
              {() => (recording() ? "Recording" : "Idle")}
            </text>
          </hstack>
        </vstack>
        <divider />

        {/* Content */}
        <vstack grow padding={24} gap={16} background="background">
          <Show when={() => page() === 0}>
            <Accent.Provider value={recording() ? "red" : "accent"}>
              <text variant="largeTitle">Overview</text>
              <grid columns={3} gap={12}>
                <Metric title="Workers" value={() => String(workers())} detail={() => "parallel lanes"} />
                <Metric title="Heap" value={heap} />
                <Metric
                  title="Throughput"
                  value={throughput}
                  detail={() => (isPending(throughput) ? "measuring…" : "steady")}
                />
              </grid>
              <card direction="row" align="center">
                <text grow>Run a quick profile with the current settings.</text>
                <Show when={() => isPending(throughput)}>
                  <spinner />
                </Show>
                <button variant={() => (recording() ? "destructive" : "primary")} onPress={() => setRecording((r) => !r)}>
                  {() => (recording() ? "Stop" : "Start Profiling")}
                </button>
              </card>
            </Accent.Provider>
          </Show>

          <Show when={() => page() === 1}>
            <text variant="largeTitle">Controls</text>
            <card>
              <hstack>
                <text grow>Workers</text>
                <text monospace color="secondary">
                  {() => String(workers()).padStart(2, " ")}
                </text>
              </hstack>
              <slider min={1} max={32} step={1} value={workers} onChange={setWorkers} />
              <progress min={0} max={32} value={workers} />
            </card>
            <card>
              <hstack>
                <text grow>Quality</text>
                <segmented options={["Draft", "Balanced", "Best"]} selected={quality} onChange={setQuality} />
              </hstack>
              <hstack>
                <text grow>Region</text>
                <select options={["us-east", "eu-west", "ap-south"]} selected={region} onChange={setRegion} />
              </hstack>
              <hstack>
                <text grow>Hardware acceleration</text>
                <toggle checked onChange={(on: boolean) => notify("Acceleration", on ? "On" : "Off")} />
              </hstack>
            </card>
            <hstack>
              <button icon="folder" onPress={async () => {
                const path = await dialog.openFile({ title: "Open a profile" });
                if (path) setNotes((n) => `${n}\nOpened ${path}`);
              }}>
                Open…
              </button>
              <button icon="doc.on.doc" onPress={() => clipboard.writeText(`${workers()} workers, ${heap()}`)}>
                Copy Summary
              </button>
            </hstack>
          </Show>

          <Show when={() => page() === 2}>
            <hstack>
              <text variant="largeTitle" grow>
                Notes
              </text>
              <text color="secondary">{() => `${words()} words`}</text>
            </hstack>
            <textarea value={notes} onChange={setNotes} />
          </Show>
        </vstack>
      </hstack>
    </window>
  );
}

runApp(<Studio />);
