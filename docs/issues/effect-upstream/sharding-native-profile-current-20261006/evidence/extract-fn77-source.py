#!/usr/bin/env python3
"""Extract the generated-source span from one module C file's concatenated _source[]."""

import json
import sys
from pathlib import Path

if len(sys.argv) != 3:
    raise SystemExit(f"usage: {sys.argv[0]} module.c output.txt")

c_path, output_path = map(Path, sys.argv[1:])
text = c_path.read_text()
marker = "kiln_module_path_d26041571cf61f0bad70365407312980_source[] = "
index = text.index(marker) + len(marker)
chunks = []
while True:
    while text[index].isspace():
        index += 1
    if text[index] != '"':
        break
    end = index + 1
    escaped = False
    while end < len(text):
        char = text[end]
        if escaped:
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == '"':
            break
        end += 1
    chunks.append(json.loads(text[index : end + 1].replace(r"\?", "?")))
    index = end + 1

source = "".join(chunks)
output_path.write_text(source[34428:38039])
