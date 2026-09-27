// Parallel-builtins issue 30, item 2. `split`'s limit holds whatever path the pieces are found by:
// a Latin1 subject, a UTF-16 subject (which used to go through the regexp split and ignore the
// limit -- `"日本,語,27".split(",", 1)` answered all three pieces), and a regexp separator on
// either. Zero answers `[]`; a limit past the piece count, a negative one (ToUint32) and NaN are
// what the specification makes of them; captures count towards the limit.
const subjects = ["日本,語,27", "a,b,c", "é,ü,ß", "", "日本"];
const separators = [",", /,/, /(,)/, "", "語"];
const limits = [undefined, 0, 1, 2, 3, 5, -1, NaN, 2.7, "2"];
for (const subject of subjects) {
  for (const separator of separators) {
    const row = [];
    for (const limit of limits) row.push(JSON.stringify(subject.split(separator, limit)));
    console.log(JSON.stringify(subject), String(separator), row.join(" "));
  }
}
