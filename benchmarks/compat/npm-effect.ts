import { Effect } from 'effect'; console.log(JSON.stringify(Effect.runSync(Effect.map(Effect.succeed(21),x=>x*2))));
