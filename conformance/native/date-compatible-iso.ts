for (const source of [
  "2024-01-01 01:00:00Z", "2020-02-01T11:17:00+1100",
  "2024-01-01T00:00:00.1Z", "2024-01-01T00:00:00.123456789Z",
  "2024-07-21T20:12:34.112546348+12:00",
  "2024-01-01T24:00:00.0001Z", "2024-01-01T00:00:00.Z",
  "2024-01-01T00:00:00.1Zjunk", "2024-01-01T00:00:00+2460",
  "2024-01-01Té1:00:00.1Z",
]) {
  const millis = Date.parse(source);
  console.log(source, millis, Object.is(new Date(source).getTime(), millis));
}
for (const time of [8_640_000_000_000_001, -8_640_000_000_000_001, Infinity, NaN, -0, 1.9, -1.9]) {
  const date = new Date(time);
  console.log(String(time), date.getTime(), Object.is(date.getTime(), -0));
}
