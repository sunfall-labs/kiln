/** @jsxImportSource kiln:gui */
// The smallest kiln:gui app: one signal, two buttons, a label that follows.
//
//   kiln run --native examples/gui-counter.tsx
import { createSignal, runApp } from "kiln:gui";

function Counter() {
  const [count, setCount] = createSignal(0);
  return (
    <window title="Counter" width={280} height={150} align="center" justify="center">
      <text variant="largeTitle">{() => count()}</text>
      <hstack>
        <button onPress={() => setCount((c) => c - 1)}>−</button>
        <button variant="primary" onPress={() => setCount((c) => c + 1)}>+</button>
      </hstack>
    </window>
  );
}

runApp(<Counter />);
