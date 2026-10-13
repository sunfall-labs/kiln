#!/usr/bin/env python3
"""Render accessible initial charts and tables from the published measurement record."""
import html
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
data = json.loads((ROOT / "data/benchmarks.json").read_text())


def rows(items, values, maximum, suffix, ranges=False, name="chart"):
    description = "; ".join(item["label"] + ": " + ((f"{value[0]:g}–{value[1]:g}" if value[0] != value[1] else f"{value[1]:g}") if ranges else f"{value:g}") + suffix for item, value in zip(items, values))
    titles = {"throughput": "Hello-world HTTP throughput", "memory": "Peak server resident memory", "scaling": "Small-service worker scaling"}
    result = [f'<svg xmlns="http://www.w3.org/2000/svg" class="chart-svg" viewBox="0 0 360 220" role="img" aria-labelledby="{name}-svg-title {name}-svg-desc">',
              f'<title id="{name}-svg-title">{html.escape(titles.get(name, "JSON warm execution · October 4 snapshot"))}</title>',
              f'<desc id="{name}-svg-desc">Bars start at zero. {html.escape(description)}. Exact values are also available in the result table.</desc>',
              f'<defs><pattern id="{name}-range" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="2" height="4" fill="#ffffff" fill-opacity=".5"/></pattern></defs>']
    for index, (item, value) in enumerate(zip(items, values)):
        low, high = value if ranges else (value, value)
        label = f"{low:g}–{high:g}" if low != high else f"{high:g}"
        y = 17 + index * 45
        color = "#b6462b" if item.get("id") == "kiln" else "#73796b"
        result.append(
            f'<g class="chart-row" data-series="{html.escape(item.get("id", item["label"]))}">'
            f'<text class="chart-label" x="0" y="{y+15}" fill="{color}" font-family="sans-serif" font-size="12">{html.escape(item["label"])}</text>'
            f'<rect x="87" y="{y}" width="172" height="22" fill="#ebece3"/>'
            f'<rect class="bar" x="87" y="{y}" width="{high / maximum * 172:.4f}" height="22" fill="{color}"/>'
            + (f'<rect x="{87+low/maximum*172:.4f}" y="{y}" width="{(high-low)/maximum*172:.4f}" height="22" fill="url(#{name}-range)"/>' if ranges else "")
            + f'<text class="chart-value" x="357" y="{y+15}" text-anchor="end" fill="{color}" font-family="monospace" font-size="12">{label}{suffix}</text></g>'
        )
    result += ['<path d="M87 12V184H259" fill="none" stroke="#a9ad9f" stroke-width="1"/>',
               '<text x="87" y="204" fill="#6c7066" font-family="monospace" font-size="10">0</text>',
               f'<text x="259" y="204" text-anchor="end" fill="#6c7066" font-family="monospace" font-size="10">{maximum:g}{suffix}</text>', '</svg>']
    return "\n".join(result)


def table(headers, entries):
    return '<div class="table-scroll" tabindex="0" role="region" aria-label="Full measurement table"><table><thead><tr>' + "".join(
        f"<th scope=\"col\">{html.escape(h)}</th>" for h in headers
    ) + "</tr></thead><tbody>" + "".join(
        '<tr><th scope="row">' + html.escape(entry[0]) + "</th>" + "".join(f"<td>{html.escape(v)}</td>" for v in entry[1:]) + "</tr>"
        for entry in entries
    ) + "</tbody></table></div>"


runtime_rows = [[r["label"] + (f' · {r["version"]}' if r["version"] else ""), *(f'{r["throughput"][w["id"]]:.1f}k' for w in data["workloads"]),
                 (f'{r["rss"][0]}–{r["rss"][1]}' if r["rss"][0] != r["rss"][1] else str(r["rss"][0])) + " MB"] for r in data["runtimes"]]
scaling_rows = [[w["label"], f'{w["hello"]:.1f}k', f'{w["service"]:.1f}k'] for w in data["scaling"]["workers"]]
fragments = {
    "throughput": rows(data["runtimes"], [r["throughput"]["hello"] for r in data["runtimes"]], data["axes"]["throughput"], "k", name="throughput"),
    "memory": rows(data["runtimes"], [r["rss"] for r in data["runtimes"]], data["axes"]["memory"], " MB", ranges=True, name="memory"),
    "scaling": rows(data["scaling"]["workers"], [w["service"] for w in data["scaling"]["workers"]], data["axes"]["scaling"], "k", name="scaling"),
    "runtime-table": table(["Runtime", "Hello world", "Small service", "1 KB JSON echo", "Peak RSS"], runtime_rows),
    "scaling-table": table(["Workers", "Hello world", "Small service"], scaling_rows),
    "benchmark-data": '<script id="benchmark-data" type="application/json">' + json.dumps(data, ensure_ascii=False).replace("<", "\\u003c") + '</script>',
}
micro = json.loads((ROOT / "data/microbenchmarks.json").read_text())
assert len(micro["rows"]) == 22
raw = json.loads((ROOT / micro["raw"]).read_text())
assert micro["rows"] == raw["rows"] and not raw["divergences"]
selected = next(r for r in micro["rows"] if r["script"] == "json.mjs")
labels = {"cpu": "Numerical work", "non-io": "Collections and runtime", "io": "I/O and buffers"}
options = []
for suite, label in labels.items():
    options.append(f'<optgroup label="{label}">')
    for r in micro["rows"]:
        if r["suite"] == suite:
            name = r["script"].removesuffix(".mjs")
            key = f'{suite}/{r["script"]}'
            options.append(f'<option value="{key}"' + (' selected' if r == selected else '') + f'>{html.escape(name)}</option>')
    options.append('</optgroup>')
entries = [[f'{r["suite"]}/{r["script"]}', *(format(r["ms"][runtime], ".4g") for runtime in ("node", "bun", "kiln-native", "kiln")), f'{r["ms"]["kiln-native"]/r["ms"]["bun"]:.2f}×'] for r in micro["rows"]]
fragments.update({
    "micro-options": "\n".join(options),
    "micro-chart": rows([{"id":"kiln","label":"kiln native"},{"label":"Bun"},{"label":"Node"}], [selected["ms"][r] for r in ("kiln-native","bun","node")], 80, " ms", name="microbench"),
    "micro-table": table(["Workload", "Node ms", "Bun ms", "Native ms", "Interpreter ms", "Native/Bun"], entries),
    "micro-data": '<script id="micro-data" type="application/json">' + json.dumps(micro, ensure_ascii=False).replace("<", "\\u003c") + '</script>',
})
(ROOT / "assets/charts/microbench.svg").write_text(fragments["micro-chart"] + "\n")

for chart in ("throughput", "memory", "scaling"):
    (ROOT / "assets/charts" / f"{chart}.svg").write_text(fragments[chart] + "\n")

page = ROOT / "index.html"
text = page.read_text()
for name, fragment in fragments.items():
    start, end = f"<!-- generated:{name} -->", f"<!-- /generated:{name} -->"
    before, rest = text.split(start, 1)
    _, after = rest.split(end, 1)
    text = before + start + "\n" + fragment + "\n" + end + after
page.write_text(text)
