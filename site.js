const commands = {
  run: ['kiln run hello.ts'],
  build: ['kiln build hello.ts', './hello'],
};
const descriptions = {
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
