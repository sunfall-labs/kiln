for (const encode of [encodeURI, encodeURIComponent]) {
  console.log(encode())
  for (const input of ["\ud800", "\udfff", "a\ud800b", "\ud800\ud800", "\udfff\ud800"]) {
    try { console.log(encode(input)) }
    catch (error) { console.log(error instanceof URIError) }
  }
  console.log(encode("\ud83d\ude00"))
  let hint
  console.log(encode({ [Symbol.toPrimitive](value) { hint = value; return "a/b" } }), hint)
  try { console.log(encode(Symbol("uri"))) }
  catch (error) { console.log(error instanceof TypeError) }
}
