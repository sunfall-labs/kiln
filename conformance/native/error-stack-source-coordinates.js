Error.prepareStackTrace = (_error, sites) => {
  return sites.map((site) =>
    `${site.getFunctionName()} ${site.getLineNumber()} ${site.getColumnNumber()}`
  ).join("|")
}

function makeError() {
  return new Error()
}

function caller() {
  return makeError()
}

const stack = caller().stack
if (!stack.includes("makeError 8 10") || !stack.includes("caller 12 10")) {
  throw new Error(`incorrect call-site coordinates: ${stack}`)
}
console.log("source coordinates ok")
