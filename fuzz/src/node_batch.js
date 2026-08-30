// The batched node reference. See `fuzz/src/node.rs`.
//
// WHY THIS EXISTS. One `node program.js` costs ~30 ms, essentially all of it process startup, and
// the interpreter it is being compared against costs ~0.1 ms. A screen built on one process per
// case is therefore 99.7% node startup, and at that price a soak large enough to find anything
// takes hours. This runs many generated programs in one process, each in its own `vm` context, and
// costs ~1 ms each.
//
// THE THING THAT WOULD MAKE IT WORTHLESS is formatting that is not node's. `console.log` does not
// print values, it *inspects* them -- `[ 1, 2 ]`, `{ p0: 'x' }`, `<1 empty item>` -- and a
// hand-written formatter that got any of that wrong would report a divergence on every program
// that logged an array. So the real `node:console` is used, writing into a `Writable` instead of
// into fd 1, and nothing here formats anything. `tests/node_reference.rs` pins that this file and
// a real `node program.js` produce identical bytes.
//
// PROTOCOL, deliberately not JSON: a generated program can print a lone surrogate, and round
// tripping one through JSON is not something worth debugging inside a fuzzing harness. Both files
// are length-prefixed byte records.
//
//   input   <count>\n  then <byte-length>\n<source bytes> per program
//   output  <count>\n  then per program:
//             <status>\n            ok | throw | invalid
//             <byte-length>\n<diagnostic bytes>
//             <byte-length>\n<stdout bytes>
//
// `invalid` means node itself refused the source -- a SyntaxError. That is a bug in the generator,
// not in kiln, and it is classified apart from everything else for exactly that reason.

'use strict';

const fs = require('node:fs');
const vm = require('node:vm');
const { Console } = require('node:console');
const { Writable } = require('node:stream');

// A program that prints without bound would take the whole batch down with it. The interpreter
// screen ahead of this one already bounds the work (50,000 steps), so this is belt and braces --
// but a batch is 64 cases and losing 63 of them to the 64th is not a trade worth making.
const MAX_OUTPUT = 1 << 20;

class Overflow extends Error {}

function readRecords(path) {
    const buffer = fs.readFileSync(path);
    let cursor = 0;
    const line = () => {
        const end = buffer.indexOf(0x0a, cursor);
        const text = buffer.toString('utf8', cursor, end);
        cursor = end + 1;
        return text;
    };
    const count = Number(line());
    const records = [];
    for (let index = 0; index < count; index += 1) {
        const length = Number(line());
        records.push(buffer.toString('utf8', cursor, cursor + length));
        cursor += length;
    }
    return records;
}

function runOne(source) {
    let captured = '';
    let overflowed = false;
    const sink = new Writable({
        write(chunk, _encoding, done) {
            captured += chunk.toString('utf8');
            if (captured.length > MAX_OUTPUT) {
                overflowed = true;
            }
            done();
        },
    });
    // The real thing. `colorMode: false` because the reference this is pinned against is
    // `node program.js` with stdout redirected to a file, which is not a TTY either.
    const console = new Console({ stdout: sink, stderr: sink, colorMode: false });

    let status = 'ok';
    let diagnostic = '';
    try {
        // A fresh context per program, so `let a, b, c, d` in one case cannot be seen by the next.
        // Well-known symbols are shared across realms, so `Symbol.toPrimitive` still means what the
        // generated program thinks it means.
        const context = vm.createContext({ console });
        // `displayErrors: false`: a syntax error's rendered code frame is noise in a report that
        // groups by reason, and the message alone is what names the generator bug.
        const script = new vm.Script(source, { filename: 'generated.js', displayErrors: false });
        script.runInContext(context, { displayErrors: false });
    } catch (error) {
        if (overflowed) {
            throw new Overflow();
        }
        // A SyntaxError from `new vm.Script` never ran a line, so nothing was printed and the
        // program was never valid JavaScript. Anything else is the program throwing, which is a
        // thing generated programs do on purpose -- `Statement::Throw` exists.
        status = error instanceof SyntaxError && captured.length === 0 ? 'invalid' : 'throw';
        try {
            diagnostic = String(error && error.stack ? error.stack.split('\n')[0] : error);
        } catch {
            diagnostic = 'a thrown value that could not be described';
        }
    }
    if (overflowed) {
        throw new Overflow();
    }
    return { status, diagnostic, stdout: captured };
}

function main() {
    const [, , inputPath, outputPath] = process.argv;
    const sources = readRecords(inputPath);
    const parts = [Buffer.from(`${sources.length}\n`, 'utf8')];
    const push = (text) => {
        const bytes = Buffer.from(text, 'utf8');
        parts.push(Buffer.from(`${bytes.length}\n`, 'utf8'), bytes);
    };
    for (const source of sources) {
        let result;
        try {
            result = runOne(source);
        } catch (error) {
            // Reported rather than swallowed: the caller re-runs an unusable batch one process at
            // a time, so a case that cannot be answered here is not a case that is lost.
            result = {
                status: 'unusable',
                diagnostic: error instanceof Overflow
                    ? `the program printed more than ${MAX_OUTPUT} bytes`
                    : String(error),
                stdout: '',
            };
        }
        parts.push(Buffer.from(`${result.status}\n`, 'utf8'));
        push(result.diagnostic);
        push(result.stdout);
    }
    fs.writeFileSync(outputPath, Buffer.concat(parts));
}

main();
