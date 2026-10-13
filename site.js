const commands = {
  check: ['kiln check hello.ts'],
  run: ['kiln run hello.ts'],
  build: ['kiln build hello.ts', './hello'],
};
const descriptions = {
  check: 'Check TypeScript source before running or compiling it.',
  run: 'The built-in interpreter runs your program immediately.',
  build: 'Compile a native executable, then run it directly.',
};
document.querySelectorAll('[data-mode]').forEach((button) => {
  button.addEventListener('click', () => {
    const mode = button.dataset.mode;
    const code = document.getElementById('workflow-command');
    code.replaceChildren();
    commands[mode].forEach((command, index) => {
      if (index) code.append(document.createTextNode('\n'));
      const prompt = document.createElement('span');
      prompt.className = 'prompt';
      prompt.textContent = '$';
      code.append(prompt, document.createTextNode(` ${command}`));
    });
    document.getElementById('workflow-description').textContent = descriptions[mode];
    document.getElementById('workflow-result').textContent = mode === 'check' ? 'no errors' : '42';
    document.querySelectorAll('[data-mode]').forEach((item) => {
      item.setAttribute('aria-pressed', String(item === button));
    });
  });
});

let toastTimer;
document.querySelectorAll('[data-copy]').forEach((button) => {
  button.addEventListener('click', async () => {
    const status = document.getElementById('copy-status');
    const code = document.getElementById(button.dataset.copy);
    try {
      await navigator.clipboard.writeText(code.textContent.trim());
      status.textContent = 'Installation command copied.';
    } catch {
      const selection = window.getSelection();
      const range = document.createRange();
      range.selectNodeContents(code);
      selection.removeAllRanges();
      selection.addRange(range);
      status.textContent = 'Command selected. Press Ctrl+C or ⌘C to copy.';
    }
    clearTimeout(toastTimer);
    status.classList.add('visible');
    toastTimer = setTimeout(() => status.classList.remove('visible'), 4000);
  });
});

const measurements = JSON.parse(document.getElementById('benchmark-data').textContent);
document.documentElement.classList.add('js-charts');

function selectChart(button, selector) {
  document.querySelectorAll(selector).forEach((item) => {
    item.setAttribute('aria-pressed', String(item === button));
  });
}

function setBars(container, values, maximum) {
  container.querySelectorAll('.chart-row').forEach((row, index) => {
    row.querySelector('.bar').setAttribute('width', values[index] / maximum * 172);
    row.querySelector('.chart-value').textContent = `${values[index].toFixed(1)}k`;
  });
  const svg = container.querySelector('svg');
  svg.querySelector('desc').textContent = [...container.querySelectorAll('.chart-row')].map(row => `${row.querySelector('.chart-label').textContent}: ${row.querySelector('.chart-value').textContent}`).join('; ');
  svg.querySelector('title').textContent = container.id === 'throughput-bars' ? 'HTTP throughput measurements' : 'Worker scaling measurements';

}

document.querySelectorAll('[data-workload]').forEach((button) => {
  button.addEventListener('click', () => {
    const workload = measurements.workloads.find((item) => item.id === button.dataset.workload);
    selectChart(button, '[data-workload]');
    setBars(document.getElementById('throughput-bars'),
      measurements.runtimes.map((runtime) => runtime.throughput[workload.id]),
      measurements.axes.throughput);
    document.querySelector('#throughput-bars svg title').textContent = `${workload.label}: HTTP throughput`;
    document.getElementById('throughput-title').textContent = workload.label;
    document.getElementById('workload-description').textContent = workload.description;
    document.getElementById('throughput-takeaway').textContent = workload.takeaway;
  });
});

document.querySelectorAll('[data-scaling]').forEach((button) => {
  button.addEventListener('click', () => {
    const workload = button.dataset.scaling;
    selectChart(button, '[data-scaling]');
    setBars(document.getElementById('scaling-bars'),
      measurements.scaling.workers.map((worker) => worker[workload]),
      measurements.axes.scaling);
    document.querySelector('#scaling-bars svg title').textContent = `${workload === 'service' ? 'Small service' : 'Hello world'}: worker scaling`;
    document.getElementById('scaling-title').textContent = workload === 'service'
      ? 'More capacity for the small service.' : 'More workers are not always faster.';
    document.getElementById('scaling-takeaway').textContent = measurements.scaling.takeaways[workload];
  });
});

const microbenchmarks = JSON.parse(document.getElementById('micro-data').textContent);
const microSelect = document.getElementById('micro-workload');
microSelect.addEventListener('change', () => {
  const row = microbenchmarks.rows.find(item => `${item.suite}/${item.script}` === microSelect.value);
  const values = ['kiln-native', 'bun', 'node'].map(runtime => row.ms[runtime]);
  const peak = Math.max(...values);
  const magnitude = 10 ** Math.floor(Math.log10(peak));
  const maximum = Math.ceil(peak / magnitude) * magnitude;
  const container = document.getElementById('micro-bars');
  container.querySelectorAll('.chart-row').forEach((bar, index) => {
    bar.querySelector('.bar').setAttribute('width', values[index] / maximum * 172);
    bar.querySelector('.chart-value').textContent = `${Number(values[index].toPrecision(4))} ms`;
  });
  const name = row.script.replace('.mjs', '').replaceAll('-', ' ');
  document.getElementById('micro-title').textContent = name;
  const svg = container.querySelector('svg');
  svg.querySelector('title').textContent = `${name}: warm execution, October 4 snapshot`;
  svg.querySelector('desc').textContent = [...container.querySelectorAll('.chart-row')]
    .map(bar => `${bar.querySelector('.chart-label').textContent}: ${bar.querySelector('.chart-value').textContent}`).join('; ');
  svg.lastElementChild.textContent = `${Number(maximum.toPrecision(4))} ms`;
  const ratio = row.ms['kiln-native'] / row.ms.bun;
  document.getElementById('micro-takeaway').textContent = ratio < 1
    ? `Native takes ${(ratio * 100).toFixed(1)}% of Bun’s time on this workload in the October 4 snapshot.`
    : `Native takes ${ratio.toFixed(2)}× Bun’s time on this workload in the October 4 snapshot.`;
  document.getElementById('micro-source').href = `https://github.com/sunfall-labs/kiln/blob/${microbenchmarks.validated_commit}/bench/${row.suite}/${row.script}`;
});
