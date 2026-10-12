'use strict';
const search = document.querySelector('.package-search');
const input = document.querySelector('#package-query');
const count = document.querySelector('#package-count');
const rows = [...document.querySelectorAll('#package-results tbody tr')];
if (search && input && count) {
  search.hidden = false;
  input.addEventListener('input', () => {
    const query = input.value.trim().toLowerCase();
    let visible = 0;
    for (const row of rows) {
      row.hidden = !row.cells[0].textContent.toLowerCase().includes(query);
      if (!row.hidden) visible++;
    }
    count.textContent = `${visible} of ${rows.length} packages${visible === 0 ? ' — no matches' : ''}`;
  });
}
