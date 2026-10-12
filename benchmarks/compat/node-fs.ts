import fs from 'node:fs'; console.log(JSON.stringify(fs.readFileSync('benchmarks/fixture.txt','utf8').trim()));
