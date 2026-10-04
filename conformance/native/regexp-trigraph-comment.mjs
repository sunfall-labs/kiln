// kiln-native: profile=native-release
// C translates ??/ to a backslash before removing comments. A raw pattern ending in ??
// therefore joined the generated helper declaration into its preceding comment.
const pathParameter = /:(\w+)\??/;
for (const input of [":name?", ":name", ":_id?", ":?", "prefix:field?tail"]) {
  const match = pathParameter.exec(input);
  console.log(input, match === null ? null : [match[0], match[1], match.index].join("|"));
}
