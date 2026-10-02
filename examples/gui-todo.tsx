/** @jsxImportSource kiln:gui */
// A to-do list: keyed lists (For), conditional UI (Show), a text field that submits on Return,
// derived counts (createMemo) and a menu with a keyboard shortcut.
//
//   kiln run --native examples/gui-todo.tsx
import { app, createMemo, createSignal, For, runApp, Show } from "kiln:gui";

interface Todo {
  id: number;
  title: string;
  done: () => boolean;
  setDone: (done: boolean) => void;
}

let nextId = 1;

function todo(title: string, done = false): Todo {
  const [isDone, setDone] = createSignal(done);
  return { id: nextId++, title, done: isDone, setDone };
}

function TodoApp() {
  const [todos, setTodos] = createSignal<Todo[]>([todo("Try kiln:gui", true), todo("Write a real app")]);
  const [draft, setDraft] = createSignal("");
  const [filter, setFilter] = createSignal(0); // all, active, done

  const remaining = createMemo(() => todos().filter((t) => !t.done()).length);
  const visible = createMemo(() => {
    const f = filter();
    return todos().filter((t) => f === 0 || (f === 1 ? !t.done() : t.done()));
  });

  const add = () => {
    const title = draft().trim();
    if (!title) return;
    setTodos((list) => [...list, todo(title)]);
    setDraft("");
  };
  const clearDone = () => setTodos((list) => list.filter((t) => !t.done()));

  app.setMenu([
    {
      label: "List",
      items: [{ label: "Clear Completed", accelerator: "CmdOrCtrl+Shift+K", onClick: clearDone }],
    },
  ]);

  return (
    <window title="To-do" width={420} height={460} minWidth={320} minHeight={300}>
      <text variant="title">To-do</text>
      <hstack>
        <input placeholder="What needs doing?" value={draft} onChange={setDraft} onSubmit={add} />
        <button variant="primary" disabled={() => draft().trim() === ""} onPress={add}>
          Add
        </button>
      </hstack>
      <segmented options={["All", "Active", "Done"]} selected={filter} onChange={setFilter} />
      <scroll>
        <For each={visible} fallback={<text color="secondary">Nothing here.</text>}>
          {(t: Todo) => (
            <hstack>
              <checkbox checked={t.done} onChange={t.setDone} />
              <text grow color={() => (t.done() ? "secondary" : "label")}>
                {t.title}
              </text>
              <button
                variant="plain"
                icon="trash"
                tooltip="Delete"
                onPress={() => setTodos((list) => list.filter((x) => x !== t))}
              />
            </hstack>
          )}
        </For>
      </scroll>
      <divider />
      <hstack>
        <text color="secondary">{() => `${remaining()} left`}</text>
        <spacer />
        <Show when={() => todos().length > remaining()}>
          <button variant="link" onPress={clearDone}>
            Clear completed
          </button>
        </Show>
      </hstack>
    </window>
  );
}

runApp(<TodoApp />);
