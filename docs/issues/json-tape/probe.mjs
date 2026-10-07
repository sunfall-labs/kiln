for (const t of ['1', '"a"', 'true', 'null', '[]', '{}', '[1,2]', '["a","b"]', '{"a":1}', '{"a":1,"b":[2,"x"],"c":{"d":null}}', '[[1],[2,[3]]]']) {
  try { const v = JSON.parse(t); console.log(t, '=>', typeof v, JSON.stringify(v), v && typeof v === 'object' ? Object.keys(v).join(',') : ''); }
  catch (e) { console.log(t, '=> threw', e.constructor.name, e.message); }
}
