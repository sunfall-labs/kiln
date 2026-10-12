// Tier A (pure compute, no imports): runs on node, bun, kiln, porffor, hermes.
//
// Product-catalog HTML render with escaping, plus an FNV-1a digest of the
// output so the checksum covers the whole document, not just its length.
type Product = { sku: string; title: string; priceCents: number };

function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function fnv1a(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  h ^= s.length;
  return h >>> 0;
}

const products: Product[] = [];
for (let i = 0; i < 2000; i++) {
  products.push({
    sku: "sku-" + i,
    title: `Widget <${i}> & "deluxe"`,
    priceCents: 199 + ((i * 37) % 20000),
  });
}

let html = "<table>";
for (const p of products) {
  const dollars = (p.priceCents / 100).toFixed(2);
  html += `<tr><td>${esc(p.sku)}</td><td>${esc(p.title)}</td><td>$${dollars}</td></tr>`;
}
html += "</table>";

console.log(`rows=${products.length} bytes=${html.length} fnv=${fnv1a(html).toString(16)}`);
console.log(html.slice(0, 120));
console.log(html.slice(-120));
